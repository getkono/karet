//! The LSP half of the connection: what `karet-jsonrpc` cannot know.
//!
//! The correlation actor — id allocation, the pending map, timeouts, the bounded
//! outbound queue, the close protocol, failing everything in flight on EOF —
//! lives in [`karet_jsonrpc`]. What stays here is the LSP-specific leaves: the
//! diagnostics fan-out, the raw-notification broadcast payload, the few
//! server→client requests a headless client must not leave hanging, and the
//! bridge that turns [`karet_jsonrpc::RpcError`] into [`LspError`].

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::broadcast;

use crate::LspError;
use crate::PublishedDiagnostics;
use crate::RawNotification;
use crate::convert;
use crate::uri;

/// The (shorter) deadline for the `shutdown` handshake and process exit.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Diagnostics broadcast capacity; slow subscribers drop the oldest sets.
const DIAGNOSTICS_CHANNEL_CAPACITY: usize = 64;

/// Every user-visible `LspError` string is produced here, so the shared actor
/// stays protocol-neutral while this crate's error surface is unchanged.
impl From<karet_jsonrpc::RpcError> for LspError {
    fn from(error: karet_jsonrpc::RpcError) -> Self {
        use karet_jsonrpc::RpcError as Rpc;
        match error {
            Rpc::Encode { method, source } => {
                Self::Protocol(format!("failed to encode {method}: {source}"))
            },
            Rpc::Decode { method, source } => Self::Protocol(format!(
                "malformed {method} response from the server: {source}"
            )),
            Rpc::Peer { method, error } => Self::Server(format!(
                "{method} failed with code {}: {}",
                error.code, error.message
            )),
            Rpc::Timeout => Self::Timeout,
            Rpc::Closed => Self::Closed,
            Rpc::QueueFull => Self::Protocol("language-server outbound queue is full".to_owned()),
            // No catch-all arm on purpose: `RpcError` is not `#[non_exhaustive]`,
            // so a variant added upstream breaks this match at compile time
            // rather than silently surfacing as the wrong `LspError` kind.
        }
    }
}

/// The LSP protocol handler: it owns the diagnostics fan-out and answers the
/// server→client requests a headless client must not leave hanging.
pub(crate) struct LspHandler {
    diagnostics: broadcast::Sender<PublishedDiagnostics>,
}

impl Default for LspHandler {
    fn default() -> Self {
        let (diagnostics, _) = broadcast::channel(DIAGNOSTICS_CHANNEL_CAPACITY);
        Self { diagnostics }
    }
}

impl karet_jsonrpc::Handler for LspHandler {
    type Framing = karet_jsonrpc::framing::content_length::ContentLength;
    /// Every server notification fans out raw — the escape hatch that lets a
    /// consumer handle server-specific methods (`language/status`,
    /// `experimental/*`) the typed surface does not model.
    type Push = RawNotification;

    const PEER: &'static str = "language server";
    // REQUEST_TIMEOUT / CLOSE_TIMEOUT / PUSH_CHANNEL_CAPACITY /
    // OUTBOUND_CHANNEL_CAPACITY all take the trait defaults, which are exactly
    // this crate's historical 30s / 5s / 64 / 256.

    fn push_payload(&self, method: &str, params: &Value) -> Option<RawNotification> {
        Some(RawNotification {
            method: method.to_owned(),
            params: params.clone(),
        })
    }

    fn on_notification(&self, method: &str, params: Value) {
        match method {
            "textDocument/publishDiagnostics" => route_diagnostics(params, &self.diagnostics),
            // Log/progress/telemetry notifications are safe to ignore headlessly.
            _ => {
                tracing::debug!(method, "raw-only server notification");
            },
        }
    }

    fn answer(&self, method: &str, params: &Value) -> Result<Value, karet_jsonrpc::ResponseError> {
        answer_server_request(method, params)
    }
}

/// A live JSON-RPC connection to one language server.
pub(crate) struct Connection(karet_jsonrpc::Connection<LspHandler>);

impl Connection {
    /// Start the reader/writer tasks over an arbitrary I/O pair.
    pub(crate) fn start<R, W>(read: R, write: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self(karet_jsonrpc::Connection::start(
            LspHandler::default(),
            read,
            write,
        ))
    }

    /// Issue `method` and await its typed result, bounded by the default
    /// request timeout.
    pub(crate) async fn request<P, T>(&self, method: &str, params: P) -> Result<T, LspError>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        Ok(self.0.request(method, params).await?)
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
        Ok(self.0.request_with(method, params, timeout).await?)
    }

    /// Send a notification (fire-and-forget).
    pub(crate) fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), LspError> {
        Ok(self.0.notify(method, params)?)
    }

    /// Subscribe to server-pushed diagnostics.
    pub(crate) fn diagnostics(&self) -> broadcast::Receiver<PublishedDiagnostics> {
        self.0.handler().diagnostics.subscribe()
    }

    /// Subscribe to every server-initiated notification, undecoded.
    pub(crate) fn raw_notifications(&self) -> broadcast::Receiver<RawNotification> {
        self.0.subscribe()
    }

    /// Drain the outbound queue, then stop both I/O tasks.
    pub(crate) async fn close(&mut self) {
        self.0.close().await;
    }
}

/// Answer the server→client requests a headless client must not leave hanging.
fn answer_server_request(
    method: &str,
    params: &Value,
) -> Result<Value, karet_jsonrpc::ResponseError> {
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
        _ => Err(karet_jsonrpc::ResponseError {
            code: karet_jsonrpc::METHOD_NOT_FOUND,
            message: format!("karet-lsp does not implement {method}"),
        }),
    }
}

/// Decode and broadcast one `textDocument/publishDiagnostics` notification.
fn route_diagnostics(params: Value, diagnostics: &broadcast::Sender<PublishedDiagnostics>) {
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
    let _ = diagnostics.send(PublishedDiagnostics {
        path,
        version: parsed.version,
        diagnostics: mapped,
    }); // no subscribers is fine
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[test]
    fn queue_full_bridges_to_a_protocol_error() -> TestResult {
        let error = LspError::from(karet_jsonrpc::RpcError::QueueFull);
        let LspError::Protocol(message) = &error else {
            return Err("expected a protocol error".into());
        };
        assert_eq!(message, "language-server outbound queue is full");
        assert_eq!(
            error.to_string(),
            "protocol error: language-server outbound queue is full"
        );
        Ok(())
    }

    #[test]
    fn encode_failures_bridge_to_a_protocol_error() -> TestResult {
        let source = serde_json::from_str::<i32>("not json")
            .err()
            .ok_or("expected a serde failure")?;
        let expected = format!("failed to encode textDocument/didOpen: {source}");
        let error = LspError::from(karet_jsonrpc::RpcError::Encode {
            method: "textDocument/didOpen".to_owned(),
            source,
        });
        let LspError::Protocol(message) = &error else {
            return Err("expected a protocol error".into());
        };
        assert_eq!(*message, expected);
        assert_eq!(error.to_string(), format!("protocol error: {expected}"));
        Ok(())
    }
}
