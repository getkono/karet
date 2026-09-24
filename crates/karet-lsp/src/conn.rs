//! The LSP half of the connection: what `karet-jsonrpc` cannot know.
//!
//! The correlation actor — id allocation, the pending map, timeouts, the bounded
//! outbound queue, the close protocol, failing everything in flight on EOF —
//! lives in [`karet_jsonrpc`]. What stays here is the LSP-specific leaves: the
//! diagnostics fan-out, the raw-notification broadcast payload, the few
//! server→client requests a headless client must not leave hanging, and the
//! bridge that turns [`karet_jsonrpc::RpcError`] into [`LspError`].

use std::sync::Arc;
use std::sync::RwLock;
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
use crate::ServerRefresh;
use crate::capability;
use crate::convert;
use crate::gate::Gate;
use crate::selector::Selector;
use crate::uri;

/// The (shorter) deadline for the `shutdown` handshake and process exit.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Diagnostics broadcast capacity; slow subscribers drop the oldest sets.
const DIAGNOSTICS_CHANNEL_CAPACITY: usize = 64;

/// Refresh broadcast capacity. A refresh carries no payload, so a subscriber
/// that lags has lost nothing but duplicates of a signal it will still see.
const REFRESH_CHANNEL_CAPACITY: usize = 16;

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
    /// Server requests to re-fetch something it answered before
    /// (`workspace/inlayHint/refresh`), fanned out to every subscriber.
    refreshes: broadcast::Sender<ServerRefresh>,
    /// What the server supports and for which documents, shared with the
    /// [`LspClient`] that gates on it.
    ///
    /// Shared rather than copied because `client/registerCapability` arrives
    /// *here*, on the handler, and has to be visible to the gate immediately.
    ///
    /// [`LspClient`]: crate::LspClient
    gate: Arc<RwLock<Gate>>,
}

impl Default for LspHandler {
    fn default() -> Self {
        let (diagnostics, _) = broadcast::channel(DIAGNOSTICS_CHANNEL_CAPACITY);
        let (refreshes, _) = broadcast::channel(REFRESH_CHANNEL_CAPACITY);
        Self {
            diagnostics,
            refreshes,
            gate: Arc::default(),
        }
    }
}

impl LspHandler {
    /// Turn on everything a `client/registerCapability` asks for, for the
    /// documents each registration's selector covers.
    fn register(&self, params: &Value) {
        let Some(items) = params.get("registrations").and_then(Value::as_array) else {
            return;
        };
        for item in items {
            let (Some(id), Some(method)) = (
                item.get("id").and_then(Value::as_str),
                item.get("method").and_then(Value::as_str),
            ) else {
                continue;
            };
            let Some(feature) = capability::feature_for_method(method) else {
                // A method karet does not gate on. Acknowledged, as before.
                continue;
            };
            let selector = Selector::from_register_options(item.get("registerOptions"));
            tracing::debug!(
                method,
                ?feature,
                ?selector,
                "server registered a capability"
            );
            if let Ok(mut gate) = self.gate.write() {
                gate.register(id.to_owned(), feature, selector);
            }
        }
    }

    /// Turn off everything a `client/unregisterCapability` withdraws.
    ///
    /// Only the named registration goes: another registration of the same
    /// method, or the handshake having advertised it, keeps the feature on.
    fn unregister(&self, params: &Value) {
        // The spec's own field name is misspelled, and servers send it that
        // way; accept the corrected spelling too rather than ignore either.
        let items = params
            .get("unregisterations")
            .or_else(|| params.get("unregistrations"))
            .and_then(Value::as_array);
        let Some(items) = items else { return };
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let feature = self
                .gate
                .write()
                .ok()
                .and_then(|mut gate| gate.unregister(id));
            if let Some(feature) = feature {
                tracing::debug!(?feature, "server unregistered a capability");
            }
        }
    }

    /// The shared gate, for the client that gates on it.
    pub(crate) fn gate(&self) -> Arc<RwLock<Gate>> {
        Arc::clone(&self.gate)
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
        match method {
            "client/registerCapability" => {
                self.register(params);
                Ok(Value::Null)
            },
            "client/unregisterCapability" => {
                self.unregister(params);
                Ok(Value::Null)
            },
            // Answered at once, before anyone has re-asked: the request only
            // tells the client its hints may be stale, and holding the reply
            // until the editor re-requests would stall the server on a
            // client that may have nothing on screen to re-ask about. No
            // subscriber is fine -- nothing is showing hints to refresh.
            "workspace/inlayHint/refresh" => {
                let _ = self.refreshes.send(ServerRefresh::InlayHints);
                Ok(Value::Null)
            },
            _ => answer_server_request(method, params),
        }
    }
}

/// A live JSON-RPC connection to one language server.
pub(crate) struct Connection(karet_jsonrpc::Connection<LspHandler>);

impl Connection {
    /// The live gate this connection's handler maintains.
    ///
    /// Shared with the handler, not a copy, so a capability registered after
    /// the handshake is visible to the gate the moment it arrives.
    pub(crate) fn gate(&self) -> Arc<RwLock<Gate>> {
        self.0.handler().gate()
    }

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

    /// Subscribe to the server's refresh requests.
    pub(crate) fn refreshes(&self) -> broadcast::Receiver<ServerRefresh> {
        self.0.handler().refreshes.subscribe()
    }

    /// Subscribe to every server-initiated notification, undecoded.
    pub(crate) fn raw_notifications(&self) -> broadcast::Receiver<RawNotification> {
        self.0.subscribe()
    }

    /// Drain the outbound queue, then stop both I/O tasks.
    pub(crate) async fn close(&mut self) {
        self.0.close().await;
    }

    /// Resolve once the connection is gone. See [`karet_jsonrpc::Connection::closed`].
    pub(crate) async fn closed(&self) {
        self.0.closed().await;
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
        // Dynamic registration is handled by `LspHandler::answer` before it
        // reaches here, because it mutates the capability set.
        "window/workDoneProgress/create" => Ok(Value::Null),
        _ => Err(karet_jsonrpc::ResponseError::new(
            karet_jsonrpc::METHOD_NOT_FOUND,
            format!("karet-lsp does not implement {method}"),
        )),
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
