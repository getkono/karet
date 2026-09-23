//! The connection actor: two I/O tasks and request/response correlation.
//!
//! A [`Connection`] owns a writer task (draining an outbound frame queue) and a
//! reader task (de-framing inbound messages and routing them): responses resolve
//! the pending request with the matching id, notifications fan out on a broadcast
//! channel as a [`Handler::Push`] payload, peer→client requests go to the
//! consumer's [`Connection::inbound_requests`] stream or, while nobody holds it,
//! to [`Handler::answer`] inline, and everything else is logged and dropped. When
//! the stream ends, in-flight requests fail with [`RpcError::Closed`].
//!
//! Everything protocol-specific lives on the [`Handler`]: the framing, the
//! broadcast payload, the answers, and the tuning constants.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::BufReader;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinHandle;

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

/// In-flight requests, keyed by the id we allocated for them.
type Pending = Arc<Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, ResponseError>>>>>;

/// The reader's half of the peer-request stream, plus whether anyone is there.
struct Inbound {
    tx: mpsc::Sender<PeerRequest>,
    active: Arc<AtomicBool>,
}

/// Every destination the reader routes a message to, bundled so the loop and
/// its frame handler each take one argument instead of a positional list.
struct Routes<H: Handler> {
    handler: Arc<H>,
    pending: Pending,
    push: broadcast::Sender<H::Push>,
    outbound: mpsc::Sender<Outbound>,
    inbound: Inbound,
}

/// An item on the outbound queue: a frame to write, or the drain-and-stop
/// signal [`Connection::close`] enqueues behind the final frames.
pub(crate) enum Outbound {
    Frame(Vec<u8>),
    Close,
}

impl std::fmt::Debug for Outbound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(frame) => write!(f, "Frame({} bytes)", frame.len()),
            Self::Close => f.write_str("Close"),
        }
    }
}

/// A live JSON-RPC connection to one peer.
pub struct Connection<H: Handler> {
    outbound: mpsc::Sender<Outbound>,
    pending: Pending,
    next_id: AtomicI64,
    push: broadcast::Sender<H::Push>,
    handler: Arc<H>,
    /// Set once either I/O task stops, so requests issued *after* the
    /// connection died fail fast with [`RpcError::Closed`] instead of sitting in
    /// the pending map until they time out.
    closed: Arc<AtomicBool>,
    /// Flipped alongside `closed`, so a caller can *await* the death rather than
    /// only observe it while issuing a request.
    ///
    /// The flag alone made loss detection demand-driven: a consumer parked on its
    /// own input with nothing to ask the peer never learned the peer had gone.
    closed_signal: watch::Sender<bool>,
    /// The peer-request stream, until a consumer takes it.
    inbound_rx: Mutex<Option<mpsc::Receiver<PeerRequest>>>,
    /// Set once the receiver has been handed out. The reader consults this
    /// rather than `Sender::is_closed`, which cannot tell "nobody has taken the
    /// receiver yet" from "a consumer is listening" — the receiver exists from
    /// the moment the channel is built.
    inbound_active: Arc<AtomicBool>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
}

impl<H: Handler> Connection<H> {
    /// Start the reader/writer tasks over an arbitrary I/O pair.
    pub fn start<R, W>(handler: H, read: R, write: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        // Both capacities are downstream-overridable consts, and both channel
        // constructors *panic* on an out-of-range value (`mpsc` on 0;
        // `broadcast` on 0 or `> usize::MAX / 2`). This crate's lint floor
        // forbids panicking library code, so a bad const degrades the capacity
        // instead of aborting the process.
        let (outbound, mut outbound_rx) =
            mpsc::channel::<Outbound>(H::OUTBOUND_CHANNEL_CAPACITY.max(1));
        let (push, _) = broadcast::channel(H::PUSH_CHANNEL_CAPACITY.clamp(1, usize::MAX / 2));
        let (inbound_tx, inbound_rx) =
            mpsc::channel::<PeerRequest>(H::INBOUND_REQUEST_CAPACITY.max(1));
        let inbound_active = Arc::new(AtomicBool::new(false));
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (closed_signal, _) = watch::channel(false);
        let handler = Arc::new(handler);

        let writer_closed = Arc::clone(&closed);
        let writer_signal = closed_signal.clone();
        let writer_task = tokio::spawn(async move {
            let mut write = write;
            while let Some(item) = outbound_rx.recv().await {
                let frame = match item {
                    Outbound::Frame(frame) => frame,
                    Outbound::Close => break,
                };
                if let Err(e) = <H::Framing as Framing>::write_frame(&mut write, &frame).await {
                    tracing::warn!(peer = H::PEER, error = %e, "peer write failed; closing writer");
                    break;
                }
            }
            writer_closed.store(true, Ordering::SeqCst);
            writer_signal.send_replace(true);
        });
        let reader_task = tokio::spawn(read_loop::<H, R>(
            BufReader::new(read),
            Routes {
                handler: Arc::clone(&handler),
                pending: Arc::clone(&pending),
                push: push.clone(),
                outbound: outbound.clone(),
                inbound: Inbound {
                    tx: inbound_tx,
                    active: Arc::clone(&inbound_active),
                },
            },
            Arc::clone(&closed),
            closed_signal.clone(),
        ));

        Self {
            outbound,
            pending,
            next_id: AtomicI64::new(1),
            push,
            handler,
            closed,
            closed_signal,
            inbound_rx: Mutex::new(Some(inbound_rx)),
            inbound_active,
            reader_task,
            writer_task,
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
    /// overflow: the peer is never left waiting on our silence.
    #[must_use]
    pub fn inbound_requests(&self) -> Option<mpsc::Receiver<PeerRequest>> {
        let taken = self.inbound_rx.lock().ok()?.take();
        if taken.is_some() {
            // Ordered after the take so the reader never routes to a stream
            // whose receiver is not yet in the consumer's hands.
            self.inbound_active.store(true, Ordering::SeqCst);
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
        let mut rx = self.closed_signal.subscribe();
        // The atomic is checked too, not just the watch value: the two are set
        // together, but a task cancelled between the store and the send would
        // otherwise leave this waiting on a signal nobody will send.
        if *rx.borrow_and_update() || self.closed.load(Ordering::SeqCst) {
            return;
        }
        // `Err` means every sender is gone, which is itself the end of the
        // connection -- so it resolves rather than propagating.
        let _ = rx.wait_for(|closed| *closed).await;
    }

    /// The protocol handler this connection was started with.
    #[must_use]
    pub fn handler(&self) -> &H {
        &self.handler
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
        let id = RequestId::Number(self.next_id.fetch_add(1, Ordering::Relaxed));
        let frame = serde_json::to_vec(&message::OutgoingRequest::new(id.clone(), method, params))
            .map_err(|source| RpcError::Encode {
                method: method.to_owned(),
                source,
            })?;
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().map_err(|_| RpcError::Closed)?;
            map.insert(id.clone(), tx);
        }
        // Checked *after* registering: if the reader exits first it drains the
        // map (failing us via the dropped sender); if it exited before our
        // insert, this flag is already set. Either way we never wait out the
        // timeout on a dead connection.
        if self.closed.load(Ordering::SeqCst) {
            self.forget(&id);
            return Err(RpcError::Closed);
        }
        if self.outbound.send(Outbound::Frame(frame)).await.is_err() {
            self.forget(&id);
            return Err(RpcError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Err(_elapsed) => {
                self.forget(&id);
                Err(RpcError::Timeout)
            },
            // The reader dropped the sender: the connection is gone.
            Ok(Err(_recv)) => Err(RpcError::Closed),
            Ok(Ok(Err(error))) => Err(RpcError::Peer {
                method: method.to_owned(),
                error,
            }),
            Ok(Ok(Ok(value))) => serde_json::from_value(value).map_err(|source| RpcError::Decode {
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
        self.outbound
            .try_send(Outbound::Frame(frame))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => RpcError::Closed,
                mpsc::error::TrySendError::Full(_) => RpcError::QueueFull,
            })
    }

    /// Subscribe to the [`Handler::Push`] payloads built from peer notifications.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<H::Push> {
        self.push.subscribe()
    }

    /// Drain the outbound queue (every already-enqueued frame is written and
    /// flushed), then stop both I/O tasks. Bounded by [`Handler::CLOSE_TIMEOUT`]
    /// in case the peer stops consuming.
    pub async fn close(&mut self) {
        let _ = self.outbound.send(Outbound::Close).await;
        let _ = tokio::time::timeout(H::CLOSE_TIMEOUT, &mut self.writer_task).await;
        self.writer_task.abort(); // no-op when it drained cleanly
        self.reader_task.abort();
    }

    /// Drop the pending entry for `id` (on timeout or send failure).
    fn forget(&self, id: &RequestId) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(id);
        }
    }
}

impl<H: Handler> Drop for Connection<H> {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

/// De-frame and route inbound messages until EOF or a framing error, then fail
/// all in-flight requests by dropping their response senders.
async fn read_loop<H, R>(
    mut reader: BufReader<R>,
    routes: Routes<H>,
    closed: Arc<AtomicBool>,
    closed_signal: watch::Sender<bool>,
) where
    H: Handler,
    R: AsyncRead + Send + Unpin + 'static,
{
    loop {
        match <H::Framing as Framing>::read_frame(&mut reader).await {
            Ok(Some(bytes)) => handle_frame::<H>(&bytes, &routes),
            Ok(None) => break,
            Err(e) => {
                // A framing error means we lost message-boundary sync; the only
                // safe recovery is to drop the connection.
                tracing::warn!(peer = H::PEER, error = %e, "peer stream lost framing; closing");
                break;
            },
        }
    }
    // Flag first, then drain: a request that raced past the flag check has
    // already registered and is failed by the drain below.
    closed.store(true, Ordering::SeqCst);
    if let Ok(mut map) = routes.pending.lock() {
        map.clear(); // dropping the senders fails the awaiting requests
    }
    // Signalled last, so anyone woken by it finds a settled connection: the flag
    // set and every in-flight request already failed. `send_replace`, not `send`,
    // because `send` discards the value when no receiver happens to exist -- and
    // the common case is that nobody is waiting at the moment the peer dies.
    closed_signal.send_replace(true);
}

/// Route one de-framed message.
fn handle_frame<H: Handler>(bytes: &[u8], routes: &Routes<H>) {
    let Routes {
        handler,
        pending,
        push,
        outbound,
        inbound,
    } = routes;
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(peer = H::PEER, error = %e, "dropping non-JSON message from the peer");
            return;
        },
    };
    match message::classify(value) {
        Some(Incoming::Response { id, result }) => {
            let sender = pending.lock().ok().and_then(|mut map| map.remove(&id));
            match sender {
                Some(sender) => {
                    let _ = sender.send(result); // requester may have timed out
                },
                None => {
                    tracing::debug!(
                        peer = H::PEER,
                        id = %id,
                        "dropping response to an unknown or abandoned request"
                    );
                },
            }
        },
        Some(Incoming::Request { id, method, params }) => {
            let responder = Responder::new(id, method.clone(), outbound.clone(), H::PEER);
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
        Some(Incoming::Notification { method, params }) => {
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
        Some(Incoming::ProtocolError { error }) => {
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
        None => {
            tracing::warn!(peer = H::PEER, "dropping a message with no JSON-RPC shape");
        },
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
