//! The DAP connection actor: two I/O tasks and request/response correlation,
//! mirroring `karet-lsp`'s connection over the same shared codec.
//!
//! DAP is not JSON-RPC: every message carries a `seq` and a `type` of
//! `request`/`response`/`event`. Responses resolve the pending request with
//! the matching `request_seq` (a `success: false` response is a normal error,
//! not a transport fault); events decode into [`DebugEvent`]s and fan out on
//! a broadcast channel; adapter→client *reverse* requests (`runInTerminal`)
//! are answered "unsupported" so no adapter waits forever. When the stream
//! ends, in-flight requests fail with [`DapError::Closed`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use karet_lsp::codec;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::BufReader;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::DapError;
use crate::types::DebugEvent;
use crate::types::event_from;

/// How long a request may wait for its response before failing with
/// [`DapError::Timeout`].
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Event broadcast capacity; slow subscribers drop the oldest events.
const EVENT_CHANNEL_CAPACITY: usize = 256;
/// Frames waiting to be written.
const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

/// A pending request's resolver: `Ok(body)` on success, the adapter's message
/// on a failed response.
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

/// The one-shot "`initialized` event seen" latch: the configuration phase may
/// begin. A flag plus [`Notify`] rather than a broadcast subscription, so the
/// signal cannot be missed between `initialize` and `start`.
#[derive(Default)]
pub(crate) struct InitializedLatch {
    flag: AtomicBool,
    notify: Notify,
}

impl InitializedLatch {
    /// Wait until the adapter has announced `initialized` (immediately when it
    /// already has).
    pub(crate) async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.flag.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    fn set(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

/// A live connection to one debug adapter.
pub(crate) struct Connection {
    outbound: mpsc::Sender<Vec<u8>>,
    pending: Pending,
    next_seq: Arc<AtomicI64>,
    events: broadcast::Sender<DebugEvent>,
    pub(crate) initialized: Arc<InitializedLatch>,
    closed: Arc<AtomicBool>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    /// The single task draining deferred refusals; see [`Refusals`].
    drainer_task: JoinHandle<()>,
}

impl Connection {
    /// Start the reader/writer tasks over an arbitrary I/O pair.
    pub(crate) fn start<R, W>(read: R, write: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (outbound, mut outbound_rx) = mpsc::channel::<Vec<u8>>(OUTBOUND_CHANNEL_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let pending: Pending = Arc::default();
        let next_seq = Arc::new(AtomicI64::new(1));
        let initialized = Arc::new(InitializedLatch::default());
        let closed = Arc::new(AtomicBool::new(false));
        let (refusals, drainer_task) = Refusals::start(outbound.clone());

        let writer_closed = Arc::clone(&closed);
        let writer_task = tokio::spawn(async move {
            let mut write = write;
            while let Some(frame) = outbound_rx.recv().await {
                if let Err(e) = codec::write_frame(&mut write, &frame).await {
                    tracing::warn!(error = %e, "debug-adapter write failed; closing writer");
                    break;
                }
            }
            writer_closed.store(true, Ordering::SeqCst);
        });
        let reader_task = tokio::spawn(read_loop(ReadLoop {
            reader: BufReader::new(read),
            pending: Arc::clone(&pending),
            events: events.clone(),
            refusals,
            next_seq: Arc::clone(&next_seq),
            initialized: Arc::clone(&initialized),
            closed: Arc::clone(&closed),
        }));

        Self {
            outbound,
            pending,
            next_seq,
            events,
            initialized,
            closed,
            reader_task,
            writer_task,
            drainer_task,
        }
    }

    /// Issue `command` and await its response body, bounded by
    /// [`REQUEST_TIMEOUT`]. A failed (`success: false`) response is a
    /// [`DapError::Adapter`] error carrying the adapter's message.
    pub(crate) async fn request(&self, command: &str, arguments: Value) -> Result<Value, DapError> {
        self.request_deferred(command, arguments).await?.await
    }

    /// Put `command` on the wire *now* and return a future for its response.
    ///
    /// The split matters for sequencing: `launch` must be sent before the
    /// client starts waiting on the `initialized` event, without awaiting its
    /// (much later) response — a lazy future inside `select!` could otherwise
    /// go unpolled and never send at all.
    pub(crate) async fn request_deferred(
        &self,
        command: &str,
        arguments: Value,
    ) -> Result<impl Future<Output = Result<Value, DapError>> + Send + use<>, DapError> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({
            "seq": seq,
            "type": "request",
            "command": command,
        });
        if !arguments.is_null()
            && let Some(object) = message.as_object_mut()
        {
            object.insert("arguments".to_owned(), arguments);
        }
        let frame = serde_json::to_vec(&message)
            .map_err(|e| DapError::Protocol(format!("failed to encode {command}: {e}")))?;
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().map_err(|_| DapError::Closed)?;
            map.insert(seq, tx);
        }
        // Checked *after* registering, exactly like the LSP twin: the reader's
        // drain fails a request that raced past the flag.
        if self.closed.load(Ordering::SeqCst) {
            self.forget(seq);
            return Err(DapError::Closed);
        }
        if self.outbound.send(frame).await.is_err() {
            self.forget(seq);
            return Err(DapError::Closed);
        }
        let pending = Arc::clone(&self.pending);
        let command = command.to_owned();
        Ok(async move {
            match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
                Err(_elapsed) => {
                    if let Ok(mut map) = pending.lock() {
                        map.remove(&seq);
                    }
                    Err(DapError::Timeout)
                },
                Ok(Err(_recv)) => Err(DapError::Closed),
                Ok(Ok(Err(message))) => {
                    Err(DapError::Adapter(format!("{command} failed: {message}")))
                },
                Ok(Ok(Ok(body))) => Ok(body),
            }
        })
    }

    /// Subscribe to adapter-pushed events.
    pub(crate) fn events(&self) -> broadcast::Receiver<DebugEvent> {
        self.events.subscribe()
    }

    /// Whether either I/O task has stopped.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Drop the pending entry for `seq` (on timeout or send failure).
    fn forget(&self, seq: i64) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(&seq);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
        self.drainer_task.abort();
    }
}

/// The reader task's wiring, bundled to keep the spawn readable.
struct ReadLoop<R> {
    reader: BufReader<R>,
    pending: Pending,
    events: broadcast::Sender<DebugEvent>,
    refusals: Refusals,
    next_seq: Arc<AtomicI64>,
    initialized: Arc<InitializedLatch>,
    closed: Arc<AtomicBool>,
}

/// De-frame and route inbound messages until EOF or a framing error, then
/// fail all in-flight requests by dropping their response senders.
async fn read_loop<R>(mut ctx: ReadLoop<R>)
where
    R: AsyncRead + Send + Unpin + 'static,
{
    loop {
        match codec::read_frame(&mut ctx.reader).await {
            Ok(Some(bytes)) => handle_frame(&bytes, &ctx),
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "debug-adapter stream lost framing; closing");
                break;
            },
        }
    }
    ctx.closed.store(true, Ordering::SeqCst);
    if let Ok(mut map) = ctx.pending.lock() {
        map.clear(); // dropping the senders fails the awaiting requests
    }
    // A session that dies before `initialized` must not hang the handshake.
    ctx.initialized.set();
    let _ = ctx.events.send(DebugEvent::Terminated);
}

/// Route one de-framed message.
fn handle_frame<R>(bytes: &[u8], ctx: &ReadLoop<R>) {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "dropping a non-JSON message from the debug adapter");
            return;
        },
    };
    match value.get("type").and_then(Value::as_str) {
        Some("response") => {
            let Some(request_seq) = value.get("request_seq").and_then(Value::as_i64) else {
                tracing::warn!("dropping a response with no request_seq");
                return;
            };
            let sender = ctx
                .pending
                .lock()
                .ok()
                .and_then(|mut map| map.remove(&request_seq));
            let Some(sender) = sender else {
                tracing::debug!(request_seq, "dropping a response to an abandoned request");
                return;
            };
            let success = value
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let outcome = if success {
                Ok(value.get("body").cloned().unwrap_or(Value::Null))
            } else {
                Err(value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request rejected")
                    .to_owned())
            };
            let _ = sender.send(outcome); // requester may have timed out
        },
        Some("event") => {
            let name = value.get("event").and_then(Value::as_str).unwrap_or("");
            let body = value.get("body").cloned().unwrap_or(Value::Null);
            if name == "initialized" {
                ctx.initialized.set();
            }
            match event_from(name, &body) {
                Some(event) => {
                    let _ = ctx.events.send(event); // no subscribers is fine
                },
                None => tracing::debug!(event = name, "unmodeled debug-adapter event"),
            }
        },
        Some("request") => {
            // A reverse request (runInTerminal). karet runs adapters headless
            // through the supervisor; declining is the honest answer, and it
            // must not be left hanging.
            let seq = ctx.next_seq.fetch_add(1, Ordering::Relaxed);
            let command = value.get("command").and_then(Value::as_str).unwrap_or("");
            let refusal = json!({
                "seq": seq,
                "type": "response",
                "request_seq": value.get("seq").and_then(Value::as_i64).unwrap_or(0),
                "command": command,
                "success": false,
                "message": format!("karet does not support the {command} reverse request"),
            });
            match serde_json::to_vec(&refusal) {
                Ok(frame) => ctx.refusals.deliver(frame, command),
                Err(e) => tracing::warn!(error = %e, "failed to encode a refusal"),
            }
        },
        _ => tracing::warn!("dropping a message with no DAP shape"),
    }
}

/// The path every reverse-request refusal takes to the writer: never dropped,
/// never blocking the reader.
///
/// `try_send` alone discarded the refusal whenever the outbound queue was
/// full, which is the one outcome that defeats the point of refusing at all:
/// the adapter blocks on its reverse request forever, and `runInTerminal` is
/// issued precisely when a session is trying to start. The wait cannot happen
/// inline either: this runs on the reader task, and an adapter can be blocked
/// writing to a stdout we have stopped draining -- waiting here would close
/// that into a deadlock.
///
/// So a refusal that finds the queue full parks on an overflow queue that
/// **one** drainer task per connection moves onto the outbound queue as
/// capacity frees, in order; while anything is parked, later refusals park
/// behind it rather than overtake it. Tasks are therefore O(1) per
/// connection. Memory is bounded by the adapter -- at most one parked refusal
/// per reverse request it sent -- not by a constant, because a hard cap would
/// have to drop a refusal or block the reader, the two outcomes above.
struct Refusals {
    outbound: mpsc::Sender<Vec<u8>>,
    overflow: mpsc::UnboundedSender<Vec<u8>>,
    /// Frames parked on `overflow` and not yet handed to `outbound`.
    backlog: Arc<AtomicUsize>,
}

impl Refusals {
    /// Build the refusal path over `outbound` and spawn its single drainer.
    fn start(outbound: mpsc::Sender<Vec<u8>>) -> (Self, JoinHandle<()>) {
        let (overflow, mut parked) = mpsc::unbounded_channel::<Vec<u8>>();
        let backlog = Arc::new(AtomicUsize::new(0));
        let drainer_outbound = outbound.clone();
        let drainer_backlog = Arc::clone(&backlog);
        let drainer = tokio::spawn(async move {
            while let Some(frame) = parked.recv().await {
                // Errors only once the writer is gone: nothing to answer then.
                let _ = drainer_outbound.send(frame).await;
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

    /// Hand `frame` to the writer without dropping it and without blocking.
    fn deliver(&self, frame: Vec<u8>, command: &str) {
        // The fast path only while nothing is parked, so a refusal never
        // overtakes one deferred before it.
        let frame = if self.backlog.load(Ordering::SeqCst) == 0 {
            match self.outbound.try_send(frame) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!(command, "adapter closed before its refusal could be sent");
                    return;
                },
                Err(mpsc::error::TrySendError::Full(frame)) => frame,
            }
        } else {
            frame
        };
        self.backlog.fetch_add(1, Ordering::SeqCst);
        if self.overflow.send(frame).is_err() {
            self.backlog.fetch_sub(1, Ordering::SeqCst);
            tracing::debug!(command, "adapter closed before its refusal could be sent");
        }
    }
}
