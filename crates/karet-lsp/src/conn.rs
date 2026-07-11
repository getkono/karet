//! The connection actor: two I/O tasks and request/response correlation.
//!
//! A [`Connection`] owns a writer task (draining an outbound frame queue) and a
//! reader task (de-framing inbound messages and routing them): responses resolve
//! the pending request with the matching id, `textDocument/publishDiagnostics`
//! fans out on a broadcast channel, the few server→client requests a headless
//! client must answer are answered inline, and everything else is logged and
//! dropped. When the stream ends, in-flight requests fail with
//! [`LspError::Closed`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use karet_core::Diagnostic;
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

use crate::LspError;
use crate::codec;
use crate::convert;
use crate::jsonrpc;
use crate::jsonrpc::Incoming;
use crate::jsonrpc::ResponseError;
use crate::uri;

/// How long a request may wait for its response before failing with
/// [`LspError::Timeout`].
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The (shorter) deadline for the `shutdown` handshake and process exit.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Diagnostics broadcast capacity; slow subscribers drop the oldest sets.
const DIAGNOSTICS_CHANNEL_CAPACITY: usize = 64;

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, ResponseError>>>>>;

/// An item on the outbound queue: a frame to write, or the drain-and-stop
/// signal [`Connection::close`] enqueues behind the final frames.
enum Outbound {
    Frame(Vec<u8>),
    Close,
}

/// A live JSON-RPC connection to one language server.
pub(crate) struct Connection {
    outbound: mpsc::UnboundedSender<Outbound>,
    pending: Pending,
    next_id: AtomicI64,
    diagnostics: broadcast::Sender<(PathBuf, Vec<Diagnostic>)>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
}

impl Connection {
    /// Start the reader/writer tasks over an arbitrary I/O pair.
    pub(crate) fn start<R, W>(read: R, write: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (outbound, mut outbound_rx) = mpsc::unbounded_channel::<Outbound>();
        let (diagnostics, _) = broadcast::channel(DIAGNOSTICS_CHANNEL_CAPACITY);
        let pending: Pending = Arc::default();

        let writer_task = tokio::spawn(async move {
            let mut write = write;
            while let Some(item) = outbound_rx.recv().await {
                let frame = match item {
                    Outbound::Frame(frame) => frame,
                    Outbound::Close => break,
                };
                if let Err(e) = codec::write_frame(&mut write, &frame).await {
                    tracing::warn!(error = %e, "language-server write failed; closing writer");
                    break;
                }
            }
        });
        let reader_task = tokio::spawn(read_loop(
            BufReader::new(read),
            Arc::clone(&pending),
            diagnostics.clone(),
            outbound.clone(),
        ));

        Self {
            outbound,
            pending,
            next_id: AtomicI64::new(1),
            diagnostics,
            reader_task,
            writer_task,
        }
    }

    /// Issue `method` and await its typed result, bounded by [`REQUEST_TIMEOUT`].
    pub(crate) async fn request<P, T>(&self, method: &str, params: P) -> Result<T, LspError>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        self.request_with(method, params, REQUEST_TIMEOUT).await
    }

    /// Issue `method` and await its typed result, bounded by `timeout`.
    pub(crate) async fn request_with<P, T>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<T, LspError>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = serde_json::to_vec(&jsonrpc::OutgoingRequest::new(id, method, params))
            .map_err(|e| LspError::Protocol(format!("failed to encode {method}: {e}")))?;
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().map_err(|_| LspError::Closed)?;
            map.insert(id, tx);
        }
        if self.outbound.send(Outbound::Frame(frame)).is_err() {
            self.forget(id);
            return Err(LspError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Err(_elapsed) => {
                self.forget(id);
                Err(LspError::Timeout)
            },
            // The reader dropped the sender: the connection is gone.
            Ok(Err(_recv)) => Err(LspError::Closed),
            Ok(Ok(Err(rpc))) => Err(LspError::Server(format!(
                "{method} failed with code {}: {}",
                rpc.code, rpc.message
            ))),
            Ok(Ok(Ok(value))) => serde_json::from_value(value).map_err(|e| {
                LspError::Protocol(format!("malformed {method} response from the server: {e}"))
            }),
        }
    }

    /// Send a notification (fire-and-forget).
    pub(crate) fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), LspError> {
        let frame = serde_json::to_vec(&jsonrpc::OutgoingNotification::new(method, params))
            .map_err(|e| LspError::Protocol(format!("failed to encode {method}: {e}")))?;
        self.outbound
            .send(Outbound::Frame(frame))
            .map_err(|_| LspError::Closed)
    }

    /// Subscribe to server-pushed diagnostics.
    pub(crate) fn diagnostics(&self) -> broadcast::Receiver<(PathBuf, Vec<Diagnostic>)> {
        self.diagnostics.subscribe()
    }

    /// Drain the outbound queue (every already-enqueued frame is written and
    /// flushed), then stop both I/O tasks. Bounded by [`SHUTDOWN_TIMEOUT`] in
    /// case the peer stops consuming.
    pub(crate) async fn close(&mut self) {
        let _ = self.outbound.send(Outbound::Close);
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut self.writer_task).await;
        self.writer_task.abort(); // no-op when it drained cleanly
        self.reader_task.abort();
    }

    /// Drop the pending entry for `id` (on timeout or send failure).
    fn forget(&self, id: i64) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(&id);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

/// De-frame and route inbound messages until EOF or a framing error, then fail
/// all in-flight requests by dropping their response senders.
async fn read_loop<R>(
    mut reader: BufReader<R>,
    pending: Pending,
    diagnostics: broadcast::Sender<(PathBuf, Vec<Diagnostic>)>,
    outbound: mpsc::UnboundedSender<Outbound>,
) where
    R: AsyncRead + Send + Unpin + 'static,
{
    loop {
        match codec::read_frame(&mut reader).await {
            Ok(Some(bytes)) => handle_frame(&bytes, &pending, &diagnostics, &outbound),
            Ok(None) => break,
            Err(e) => {
                // A framing error means we lost message-boundary sync; the only
                // safe recovery is to drop the connection.
                tracing::warn!(error = %e, "language-server stream lost framing; closing");
                break;
            },
        }
    }
    if let Ok(mut map) = pending.lock() {
        map.clear(); // dropping the senders fails the awaiting requests
    }
}

/// Route one de-framed message.
fn handle_frame(
    bytes: &[u8],
    pending: &Pending,
    diagnostics: &broadcast::Sender<(PathBuf, Vec<Diagnostic>)>,
    outbound: &mpsc::UnboundedSender<Outbound>,
) {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "dropping non-JSON message from the language server");
            return;
        },
    };
    match jsonrpc::classify(value) {
        Some(Incoming::Response { id, result }) => {
            let sender = pending.lock().ok().and_then(|mut map| map.remove(&id));
            match sender {
                Some(sender) => {
                    let _ = sender.send(result); // requester may have timed out
                },
                None => {
                    tracing::debug!(id, "dropping response to an unknown or abandoned request");
                },
            }
        },
        Some(Incoming::Request { id, method, params }) => {
            let outcome = answer_server_request(&method, &params);
            match serde_json::to_vec(&jsonrpc::OutgoingResponse::new(id, outcome)) {
                Ok(frame) => {
                    let _ = outbound.send(Outbound::Frame(frame));
                },
                Err(e) => {
                    tracing::warn!(error = %e, method, "failed to encode a response");
                },
            }
        },
        Some(Incoming::Notification { method, params }) => match method.as_str() {
            "textDocument/publishDiagnostics" => route_diagnostics(params, diagnostics),
            // Log/progress/telemetry notifications are safe to ignore headlessly.
            _ => {
                tracing::debug!(method, "ignoring server notification");
            },
        },
        None => {
            tracing::warn!("dropping a message with no JSON-RPC shape");
        },
    }
}

/// Answer the server→client requests a headless client must not leave hanging.
fn answer_server_request(method: &str, params: &Value) -> Result<Value, ResponseError> {
    match method {
        // No configuration to offer: answer `null` per requested item.
        "workspace/configuration" => {
            let items = params
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Ok(Value::Array(vec![Value::Null; items]))
        },
        // Acknowledge without acting; dynamic registration and progress tokens
        // carry no state a headless completion client needs.
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => Ok(Value::Null),
        _ => Err(ResponseError {
            code: jsonrpc::METHOD_NOT_FOUND,
            message: format!("karet-lsp does not implement {method}"),
        }),
    }
}

/// Decode and broadcast one `textDocument/publishDiagnostics` notification.
fn route_diagnostics(params: Value, diagnostics: &broadcast::Sender<(PathBuf, Vec<Diagnostic>)>) {
    let parsed: lsp_types::PublishDiagnosticsParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "dropping malformed publishDiagnostics");
            return;
        },
    };
    let Some(path) = uri::uri_to_path(&parsed.uri) else {
        tracing::debug!(uri = %parsed.uri.as_str(), "ignoring diagnostics for a non-file URI");
        return;
    };
    let mapped = parsed
        .diagnostics
        .into_iter()
        .map(convert::diagnostic_from_lsp)
        .collect();
    let _ = diagnostics.send((path, mapped)); // no subscribers is fine
}
