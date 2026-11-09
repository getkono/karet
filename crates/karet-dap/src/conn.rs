//! The DAP connection: `karet-jsonrpc`'s correlation [`Actor`] speaking the
//! DAP envelope.
//!
//! The actor owns the I/O tasks, the pending map, the request timeout, the
//! bounded outbound queue and fail-all-on-EOF; this module supplies only what
//! DAP means by a message. DAP is not JSON-RPC: every message carries a `seq`
//! and a `type` of `request`/`response`/`event`. Responses resolve the pending
//! request with the matching `request_seq` (a `success: false` response is a
//! normal error, not a transport fault); events decode into [`DebugEvent`]s
//! and fan out on a broadcast channel; adapter→client *reverse* requests
//! (`runInTerminal`) are answered "unsupported" so no adapter waits forever.
//! When the stream ends, in-flight requests fail with [`DapError::Closed`].

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use karet_jsonrpc::Actor;
use karet_jsonrpc::Classified;
use karet_jsonrpc::Envelope;
use karet_jsonrpc::Link;
use karet_jsonrpc::RpcError;
use karet_jsonrpc::framing::content_length::ContentLength;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::Notify;
use tokio::sync::broadcast;

use crate::DapError;
use crate::types::DebugEvent;
use crate::types::event_from;

/// Event broadcast capacity; slow subscribers drop the oldest events.
const EVENT_CHANNEL_CAPACITY: usize = 256;

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

/// An inbound DAP message that is not a response.
enum DapMessage {
    /// An adapter-pushed event.
    Event { name: String, body: Value },
    /// An adapter→client reverse request.
    Request { seq: i64, command: String },
}

/// The DAP envelope: `seq`-numbered messages, responses keyed by
/// `request_seq`, and where events and reverse requests go.
struct Dap {
    events: broadcast::Sender<DebugEvent>,
    initialized: Arc<InitializedLatch>,
}

impl Envelope for Dap {
    type Framing = ContentLength;
    type Id = i64;
    /// `Ok(body)` on success, the adapter's message on a failed response.
    type Outcome = Result<Value, String>;
    type Message = DapMessage;

    const PEER: &'static str = "debug adapter";

    fn request_id(seq: i64) -> i64 {
        seq
    }

    fn encode_request<P: Serialize>(
        &self,
        seq: &i64,
        command: &str,
        arguments: P,
    ) -> Result<Vec<u8>, serde_json::Error> {
        let arguments = serde_json::to_value(arguments)?;
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
        serde_json::to_vec(&message)
    }

    fn classify(&self, value: Value) -> Classified<i64, Self::Outcome, DapMessage> {
        let text = |key: &str| value.get(key).and_then(Value::as_str);
        match text("type") {
            Some("response") => {
                let Some(request_seq) = value.get("request_seq").and_then(Value::as_i64) else {
                    return Classified::Malformed("a response with no request_seq");
                };
                let success = value
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let outcome = if success {
                    Ok(value.get("body").cloned().unwrap_or(Value::Null))
                } else {
                    Err(text("message").unwrap_or("request rejected").to_owned())
                };
                Classified::Response {
                    id: request_seq,
                    outcome,
                }
            },
            Some("event") => Classified::Message(DapMessage::Event {
                name: text("event").unwrap_or("").to_owned(),
                body: value.get("body").cloned().unwrap_or(Value::Null),
            }),
            Some("request") => Classified::Message(DapMessage::Request {
                seq: value.get("seq").and_then(Value::as_i64).unwrap_or(0),
                command: text("command").unwrap_or("").to_owned(),
            }),
            _ => Classified::Malformed("a message with no DAP shape"),
        }
    }

    fn route(&self, message: DapMessage, link: &Link) {
        match message {
            DapMessage::Event { name, body } => {
                if name == "initialized" {
                    self.initialized.set();
                }
                match event_from(&name, &body) {
                    Some(event) => {
                        let _ = self.events.send(event); // no subscribers is fine
                    },
                    None => tracing::debug!(event = name, "unmodeled debug-adapter event"),
                }
            },
            DapMessage::Request { seq, command } => {
                // A reverse request (runInTerminal). karet runs adapters
                // headless through the supervisor; declining is the honest
                // answer, and it must not be left hanging -- so it takes the
                // link, which never drops a frame and never blocks the reader.
                let refusal = json!({
                    "seq": link.next_seq(),
                    "type": "response",
                    "request_seq": seq,
                    "command": command,
                    "success": false,
                    "message": format!("karet does not support the {command} reverse request"),
                });
                match serde_json::to_vec(&refusal) {
                    Ok(frame) => link.reply(frame),
                    Err(e) => tracing::warn!(error = %e, "failed to encode a refusal"),
                }
            },
        }
    }

    fn on_closed(&self) {
        // A session that dies before `initialized` must not hang the handshake.
        self.initialized.set();
        let _ = self.events.send(DebugEvent::Terminated);
    }
}

/// A live connection to one debug adapter.
pub(crate) struct Connection {
    actor: Actor<Dap>,
    pub(crate) initialized: Arc<InitializedLatch>,
}

impl Connection {
    /// Start the reader/writer tasks over an arbitrary I/O pair.
    pub(crate) fn start<R, W>(read: R, write: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let initialized = Arc::new(InitializedLatch::default());
        let envelope = Dap {
            events,
            initialized: Arc::clone(&initialized),
        };
        Self {
            actor: Actor::start(envelope, read, write),
            initialized,
        }
    }

    /// Issue `command` and await its response body, bounded by the actor's
    /// request timeout. A failed (`success: false`) response is a
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
        let response = self
            .actor
            .dispatch(command, arguments, Dap::REQUEST_TIMEOUT)
            .await
            .map_err(dap_error)?;
        let command = command.to_owned();
        Ok(async move {
            match response.await.map_err(dap_error)? {
                Ok(body) => Ok(body),
                Err(message) => Err(DapError::Adapter(format!("{command} failed: {message}"))),
            }
        })
    }

    /// Subscribe to adapter-pushed events.
    pub(crate) fn events(&self) -> broadcast::Receiver<DebugEvent> {
        self.actor.envelope().events.subscribe()
    }

    /// Whether either I/O task has stopped.
    pub(crate) fn is_closed(&self) -> bool {
        self.actor.is_closed()
    }
}

/// Translate an actor failure into the DAP client's error vocabulary.
///
/// Exhaustive on purpose: a variant `karet-jsonrpc` adds must be placed here
/// rather than fall into a catch-all as the wrong kind.
fn dap_error(error: RpcError) -> DapError {
    match error {
        RpcError::Closed => DapError::Closed,
        RpcError::Timeout => DapError::Timeout,
        RpcError::Encode { method, source } => {
            DapError::Protocol(format!("failed to encode {method}: {source}"))
        },
        RpcError::Decode { method, source } => {
            DapError::Protocol(format!("malformed {method} response: {source}"))
        },
        RpcError::Peer { method, error } => {
            DapError::Adapter(format!("{method} failed: {}", error.message))
        },
        RpcError::QueueFull => DapError::Protocol("the outbound queue is full".to_owned()),
    }
}
