//! The correlation actor, generic over the wire envelope.
//!
//! An [`Actor`] owns a writer task (draining a bounded outbound frame queue)
//! and a reader task (de-framing inbound messages). It allocates request ids,
//! correlates each response with its request through a pending map, bounds
//! every request with a timeout, and fails every in-flight request when the
//! stream ends. None of that depends on what a message looks like, so the
//! [`Envelope`] says it: how an id is built, how a request is encoded, and how
//! an inbound message [classifies](Envelope::classify) — a response to resolve,
//! or a message the envelope [routes](Envelope::route) itself.
//!
//! JSON-RPC 2.0 is one envelope ([`Connection`](crate::Connection) is built on
//! it); the Debug Adapter Protocol, whose responses carry a `request_seq`
//! rather than a JSON-RPC `id`, is another.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::connection::RpcError;
use crate::framing::Framing;
use crate::peer::Replies;

/// The protocol-specific half of an [`Actor`]: the framing, the request id
/// and its encoding, and what an inbound message is.
///
/// [`route`](Self::route) and [`on_closed`](Self::on_closed) run on the reader
/// task, so neither may block: anything that must await belongs on a channel
/// the envelope hands to its consumer.
pub trait Envelope: Send + Sync + 'static {
    /// How message bodies are delimited on the wire.
    type Framing: Framing;
    /// The key correlating a response with the request it answers.
    type Id: Eq + Hash + Clone + fmt::Display + Send + 'static;
    /// What a response resolves its request with.
    type Outcome: Send + 'static;
    /// An inbound message that is not a response, handed to
    /// [`route`](Self::route).
    type Message;

    /// What the peer is called in log messages.
    const PEER: &'static str = "peer";
    /// How long [`Actor::request`] waits for a response before timing out.
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
    /// The deadline for draining the outbound queue in [`Actor::close`].
    const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
    /// Frames waiting to be written. A buggy producer cannot grow memory
    /// without bound: requests wait for capacity and
    /// [`Actor::try_send`] fails fast.
    ///
    /// Frames the reader produces through [`Link::reply`] are the exception,
    /// because none may be dropped and the reader must not block: one that
    /// finds this queue full waits on a per-actor overflow queue behind a
    /// single drainer task. That overflow queue has no constant bound — a peer
    /// that floods requests while never reading grows it until it reads or the
    /// actor ends.
    ///
    /// [`Actor::start`] clamps this to at least `1`, the minimum
    /// `tokio::sync::mpsc` accepts, rather than panicking on `0`.
    const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

    /// The id for the request numbered `seq` (allocated from `1` upwards,
    /// shared with [`Link::next_seq`]).
    fn request_id(seq: i64) -> Self::Id;

    /// Encode the request `method(params)` under `id` into one frame body.
    ///
    /// # Errors
    ///
    /// The serde failure when `params` will not serialize.
    fn encode_request<P: Serialize>(
        &self,
        id: &Self::Id,
        method: &str,
        params: P,
    ) -> Result<Vec<u8>, serde_json::Error>;

    /// Classify one inbound JSON message.
    fn classify(&self, value: Value) -> Classified<Self::Id, Self::Outcome, Self::Message>;

    /// Handle one inbound message that is not a response, on the reader task.
    ///
    /// `link` writes frames the reader owes the peer (replies to its requests)
    /// without blocking and without dropping them.
    fn route(&self, message: Self::Message, link: &Link);

    /// React to the end of the inbound stream, after every in-flight request
    /// has been failed and before [`Actor::closed`] resolves.
    fn on_closed(&self) {}
}

/// What one inbound message is, as [`Envelope::classify`] reads it.
#[derive(Debug)]
pub enum Classified<Id, Outcome, Message> {
    /// The answer to the request we sent under `id`.
    Response {
        /// The id of the request it answers.
        id: Id,
        /// What the request resolves with.
        outcome: Outcome,
    },
    /// Anything else the protocol defines, for [`Envelope::route`].
    Message(Message),
    /// Not a message of this protocol; logged and dropped. The string names
    /// what was wrong with it, for the log.
    Malformed(&'static str),
}

/// The reader's write side: the never-dropping path for frames the reader owes
/// the peer, and the sequence counter request ids are allocated from.
#[derive(Clone, Debug)]
pub struct Link {
    replies: Replies,
    next_seq: Arc<AtomicI64>,
    peer: &'static str,
}

impl Link {
    /// Hand `frame` to the writer without blocking and without dropping it.
    ///
    /// It is written in order behind every frame handed over before it. Once
    /// the writer has stopped, there is no wire left and the frame is
    /// discarded (logged at `debug`).
    pub fn reply(&self, frame: Vec<u8>) {
        self.replies.deliver(Outbound::Frame(frame), self.peer);
    }

    /// Allocate the next sequence number from the counter request ids come
    /// from, for a protocol that numbers every message it sends.
    #[must_use]
    pub fn next_seq(&self) -> i64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// The reply path itself, for a token that answers later.
    pub(crate) fn replies(&self) -> &Replies {
        &self.replies
    }
}

/// An item on the outbound queue: a frame to write, or the drain-and-stop
/// signal [`Actor::close`] enqueues behind the final frames.
pub(crate) enum Outbound {
    Frame(Vec<u8>),
    Close,
}

impl fmt::Debug for Outbound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame(frame) => write!(f, "Frame({} bytes)", frame.len()),
            Self::Close => f.write_str("Close"),
        }
    }
}

/// In-flight requests, keyed by the id we allocated for them.
type Pending<E> =
    Arc<Mutex<HashMap<<E as Envelope>::Id, oneshot::Sender<<E as Envelope>::Outcome>>>>;

/// A live connection to one peer, correlating requests with responses.
pub struct Actor<E: Envelope> {
    outbound: mpsc::Sender<Outbound>,
    pending: Pending<E>,
    link: Link,
    envelope: Arc<E>,
    /// Set once either I/O task stops, so requests issued *after* the actor
    /// died fail fast with [`RpcError::Closed`] instead of sitting in the
    /// pending map until they time out.
    closed: Arc<AtomicBool>,
    /// Flipped alongside `closed`, so a caller can *await* the death rather
    /// than only observe it while issuing a request.
    closed_signal: watch::Sender<bool>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    /// The single task draining deferred replies; see [`Replies`].
    drainer_task: JoinHandle<()>,
}

impl<E: Envelope> Actor<E> {
    /// Start the reader/writer tasks over an arbitrary I/O pair.
    ///
    /// Must be called inside a Tokio runtime.
    pub fn start<R, W>(envelope: E, read: R, write: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (outbound, mut outbound_rx) =
            mpsc::channel::<Outbound>(E::OUTBOUND_CHANNEL_CAPACITY.max(1));
        let pending: Pending<E> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (closed_signal, _) = watch::channel(false);
        let envelope = Arc::new(envelope);
        let (replies, drainer_task) = Replies::start(outbound.clone(), E::PEER);
        let link = Link {
            replies,
            next_seq: Arc::new(AtomicI64::new(1)),
            peer: E::PEER,
        };

        let writer_closed = Arc::clone(&closed);
        let writer_signal = closed_signal.clone();
        let writer_task = tokio::spawn(async move {
            let mut write = write;
            while let Some(item) = outbound_rx.recv().await {
                let frame = match item {
                    Outbound::Frame(frame) => frame,
                    Outbound::Close => break,
                };
                if let Err(e) = <E::Framing as Framing>::write_frame(&mut write, &frame).await {
                    tracing::warn!(peer = E::PEER, error = %e, "peer write failed; closing writer");
                    break;
                }
            }
            writer_closed.store(true, Ordering::SeqCst);
            writer_signal.send_replace(true);
        });
        let reader_task = tokio::spawn(read_loop(
            BufReader::new(read),
            Reader {
                envelope: Arc::clone(&envelope),
                pending: Arc::clone(&pending),
                link: link.clone(),
                closed: Arc::clone(&closed),
                closed_signal: closed_signal.clone(),
            },
        ));

        Self {
            outbound,
            pending,
            link,
            envelope,
            closed,
            closed_signal,
            reader_task,
            writer_task,
            drainer_task,
        }
    }

    /// The envelope this actor was started with.
    #[must_use]
    pub fn envelope(&self) -> &E {
        &self.envelope
    }

    /// Put `method` on the wire *now* and return a future for its outcome,
    /// bounded by `timeout` from this call.
    ///
    /// The split matters when the response arrives much later than the caller
    /// can afford to wait inline: a lazy request inside a `select!` could go
    /// unpolled and never be sent at all.
    ///
    /// # Errors
    ///
    /// [`RpcError::Encode`] if `params` will not serialize and
    /// [`RpcError::Closed`] if the actor is dead; the returned future fails
    /// with [`RpcError::Timeout`] if the peer does not answer in time and
    /// [`RpcError::Closed`] if the actor dies first.
    pub async fn dispatch<P: Serialize>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<impl Future<Output = Result<E::Outcome, RpcError>> + Send + use<E, P>, RpcError>
    {
        let id = E::request_id(self.link.next_seq());
        let frame = self
            .envelope
            .encode_request(&id, method, params)
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
        // timeout on a dead actor.
        if self.closed.load(Ordering::SeqCst) {
            forget::<E>(&self.pending, &id);
            return Err(RpcError::Closed);
        }
        if self.outbound.send(Outbound::Frame(frame)).await.is_err() {
            forget::<E>(&self.pending, &id);
            return Err(RpcError::Closed);
        }
        let pending = Arc::clone(&self.pending);
        Ok(async move {
            match tokio::time::timeout(timeout, rx).await {
                Err(_elapsed) => {
                    forget::<E>(&pending, &id);
                    Err(RpcError::Timeout)
                },
                // The reader dropped the sender: the actor is gone.
                Ok(Err(_recv)) => Err(RpcError::Closed),
                Ok(Ok(outcome)) => Ok(outcome),
            }
        })
    }

    /// Issue `method` and await its outcome, bounded by
    /// [`Envelope::REQUEST_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// See [`Actor::dispatch`].
    pub async fn request<P: Serialize>(
        &self,
        method: &str,
        params: P,
    ) -> Result<E::Outcome, RpcError> {
        self.request_with(method, params, E::REQUEST_TIMEOUT).await
    }

    /// Issue `method` and await its outcome, bounded by `timeout`.
    ///
    /// # Errors
    ///
    /// See [`Actor::dispatch`].
    pub async fn request_with<P: Serialize>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<E::Outcome, RpcError> {
        self.dispatch(method, params, timeout).await?.await
    }

    /// Queue an already-encoded frame without waiting for capacity
    /// (fire-and-forget messages such as notifications).
    ///
    /// # Errors
    ///
    /// [`RpcError::Closed`] if the writer is gone, or [`RpcError::QueueFull`]
    /// if the bounded outbound queue has no room.
    pub fn try_send(&self, frame: Vec<u8>) -> Result<(), RpcError> {
        self.outbound
            .try_send(Outbound::Frame(frame))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => RpcError::Closed,
                mpsc::error::TrySendError::Full(_) => RpcError::QueueFull,
            })
    }

    /// Whether either I/O task has stopped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Resolve once this actor is gone, whether the peer hung up, the stream
    /// lost framing, or a write failed.
    ///
    /// Returns immediately for an actor that is already closed, so a caller
    /// cannot miss the transition by racing it.
    pub async fn closed(&self) {
        let mut rx = self.closed_signal.subscribe();
        // The atomic is checked too, not just the watch value: the two are set
        // together, but a task cancelled between the store and the send would
        // otherwise leave this waiting on a signal nobody will send.
        if *rx.borrow_and_update() || self.closed.load(Ordering::SeqCst) {
            return;
        }
        // `Err` means every sender is gone, which is itself the end of the
        // actor -- so it resolves rather than propagating.
        let _ = rx.wait_for(|closed| *closed).await;
    }

    /// Drain the outbound queue, then stop both I/O tasks, bounded by
    /// [`Envelope::CLOSE_TIMEOUT`] in case the peer stops consuming.
    ///
    /// The close signal travels the [`Link::reply`] path, so every reply handed
    /// over before this call is written ahead of it; one handed over after it
    /// is dropped.
    pub async fn close(&mut self) {
        self.link.replies.deliver(Outbound::Close, E::PEER);
        let _ = tokio::time::timeout(E::CLOSE_TIMEOUT, &mut self.writer_task).await;
        self.writer_task.abort(); // no-op when it drained cleanly
        self.reader_task.abort();
        self.drainer_task.abort();
    }

    /// How many requests are waiting for a response.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.pending.lock().map_or(0, |map| map.len())
    }
}

impl<E: Envelope> Drop for Actor<E> {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
        self.drainer_task.abort();
    }
}

/// Drop the pending entry for `id` (on timeout or send failure).
fn forget<E: Envelope>(pending: &Pending<E>, id: &E::Id) {
    if let Ok(mut map) = pending.lock() {
        map.remove(id);
    }
}

/// Everything the reader task routes to.
struct Reader<E: Envelope> {
    envelope: Arc<E>,
    pending: Pending<E>,
    link: Link,
    closed: Arc<AtomicBool>,
    closed_signal: watch::Sender<bool>,
}

/// De-frame and route inbound messages until EOF or a framing error, then fail
/// all in-flight requests by dropping their response senders.
async fn read_loop<E, R>(mut reader: BufReader<R>, ctx: Reader<E>)
where
    E: Envelope,
    R: AsyncRead + Send + Unpin + 'static,
{
    loop {
        match <E::Framing as Framing>::read_frame(&mut reader).await {
            Ok(Some(bytes)) => handle_frame(&bytes, &ctx),
            Ok(None) => break,
            Err(e) => {
                // A framing error means we lost message-boundary sync; the only
                // safe recovery is to drop the connection.
                tracing::warn!(peer = E::PEER, error = %e, "peer stream lost framing; closing");
                break;
            },
        }
    }
    // Flag first, then drain: a request that raced past the flag check has
    // already registered and is failed by the drain below.
    ctx.closed.store(true, Ordering::SeqCst);
    if let Ok(mut map) = ctx.pending.lock() {
        map.clear(); // dropping the senders fails the awaiting requests
    }
    ctx.envelope.on_closed();
    // Signalled last, so anyone woken by it finds a settled actor: the flag set
    // and every in-flight request already failed. `send_replace`, not `send`,
    // because `send` discards the value when no receiver happens to exist --
    // and the common case is that nobody is waiting when the peer dies.
    ctx.closed_signal.send_replace(true);
}

/// Route one de-framed message.
fn handle_frame<E: Envelope>(bytes: &[u8], ctx: &Reader<E>) {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(peer = E::PEER, error = %e, "dropping non-JSON message from the peer");
            return;
        },
    };
    match ctx.envelope.classify(value) {
        Classified::Response { id, outcome } => {
            let sender = ctx.pending.lock().ok().and_then(|mut map| map.remove(&id));
            match sender {
                Some(sender) => {
                    let _ = sender.send(outcome); // requester may have timed out
                },
                None => {
                    tracing::debug!(
                        peer = E::PEER,
                        id = %id,
                        "dropping response to an unknown or abandoned request"
                    );
                },
            }
        },
        Classified::Message(message) => ctx.envelope.route(message, &ctx.link),
        Classified::Malformed(what) => {
            tracing::warn!(peer = E::PEER, "dropping {what}");
        },
    }
}

#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
