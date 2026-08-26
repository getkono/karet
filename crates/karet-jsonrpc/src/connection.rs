//! The connection actor: two I/O tasks and request/response correlation.
//!
//! A [`Connection`] owns a writer task (draining an outbound frame queue) and a
//! reader task (de-framing inbound messages and routing them): responses resolve
//! the pending request with the matching id, notifications fan out on a broadcast
//! channel as a [`Handler::Push`] payload, peer→client requests are answered
//! inline by [`Handler::answer`], and everything else is logged and dropped. When
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
use tokio::task::JoinHandle;

use crate::framing::Framing;
use crate::message;
use crate::message::Incoming;
use crate::message::RequestId;
use crate::message::ResponseError;

/// The protocol-specific half of a connection: framing, the broadcast payload,
/// peer-request answers, and notification side effects.
///
/// Peer→client requests are answered **synchronously** on the reader task, which
/// is all a headless client needs (`workspace/configuration`,
/// `client/registerCapability`, …). A protocol whose answers must await user
/// input gains an additive, defaulted async entry point later; that is not a
/// breaking change to this trait's shape.
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
    const PUSH_CHANNEL_CAPACITY: usize = 64;
    /// Frames waiting to be written. A buggy producer cannot grow memory without
    /// bound; requests wait for capacity and notifications fail fast.
    const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

    /// Build the broadcast payload for one peer notification (`None` drops it).
    ///
    /// Called only while at least one subscriber is listening, so an expensive
    /// clone is not paid for when nobody would receive it.
    fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push>;

    /// React to one peer notification, after the broadcast fan-out.
    fn on_notification(&self, method: &str, params: Value) {
        let _ = (method, params);
    }

    /// Answer a peer→client request.
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
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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

/// An item on the outbound queue: a frame to write, or the drain-and-stop
/// signal [`Connection::close`] enqueues behind the final frames.
enum Outbound {
    Frame(Vec<u8>),
    Close,
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
        let (outbound, mut outbound_rx) = mpsc::channel::<Outbound>(H::OUTBOUND_CHANNEL_CAPACITY);
        let (push, _) = broadcast::channel(H::PUSH_CHANNEL_CAPACITY);
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);

        let writer_closed = Arc::clone(&closed);
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
        });
        let reader_task = tokio::spawn(read_loop::<H, R>(
            BufReader::new(read),
            Arc::clone(&handler),
            Arc::clone(&pending),
            push.clone(),
            outbound.clone(),
            Arc::clone(&closed),
        ));

        Self {
            outbound,
            pending,
            next_id: AtomicI64::new(1),
            push,
            handler,
            closed,
            reader_task,
            writer_task,
        }
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
    handler: Arc<H>,
    pending: Pending,
    push: broadcast::Sender<H::Push>,
    outbound: mpsc::Sender<Outbound>,
    closed: Arc<AtomicBool>,
) where
    H: Handler,
    R: AsyncRead + Send + Unpin + 'static,
{
    loop {
        match <H::Framing as Framing>::read_frame(&mut reader).await {
            Ok(Some(bytes)) => handle_frame::<H>(&bytes, &handler, &pending, &push, &outbound),
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
    if let Ok(mut map) = pending.lock() {
        map.clear(); // dropping the senders fails the awaiting requests
    }
}

/// Route one de-framed message.
fn handle_frame<H: Handler>(
    bytes: &[u8],
    handler: &H,
    pending: &Pending,
    push: &broadcast::Sender<H::Push>,
    outbound: &mpsc::Sender<Outbound>,
) {
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
            let outcome = handler.answer(&method, &params);
            match serde_json::to_vec(&message::OutgoingResponse::new(id, outcome)) {
                Ok(frame) => {
                    if outbound.try_send(Outbound::Frame(frame)).is_err() {
                        tracing::warn!(
                            peer = H::PEER,
                            method,
                            "dropping peer-request response: outbound queue full"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(peer = H::PEER, error = %e, method, "failed to encode a response");
                },
            }
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
        None => {
            tracing::warn!(peer = H::PEER, "dropping a message with no JSON-RPC shape");
        },
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
