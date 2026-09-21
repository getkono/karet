//! Relaying what a connected server pushes: diagnostics, and the status
//! notifications some servers use in place of them.
//!
//! Split out of `runtime`, which owns the task's own lifecycle. This is the one
//! piece that outlives a single loop iteration -- it runs as its own task for as
//! long as the connection does, and is aborted with it.

use karet_lsp::LspClient;
use tokio::sync::mpsc;

use super::message::LspUpdate;
use super::slot::SlotKey;

/// Relay one connection's pushes, tagged with the slot that owns them.
///
/// `key` is both halves of the identity the two messages need: the diagnostic
/// layer to publish under, and the provider to attribute a status line to.
/// They used to arrive as two separate strings -- one confusingly named
/// `language` while holding the slot key -- which is how a clear could be built
/// for a layer that did not exist.
pub(super) fn forward_diagnostics(
    client: &LspClient,
    updates: mpsc::UnboundedSender<LspUpdate>,
    key: SlotKey,
    token: u64,
) -> tokio::task::JoinHandle<()> {
    let mut diagnostic_rx = client.diagnostics();
    let mut raw_rx = client.raw_notifications();
    let status_updates = updates.clone();
    let status_key = key.clone();
    // jdtls-style `language/status` notifications carry the only feedback a
    // user gets during a 30–120 s first import; forward them for the status
    // bar rather than leaving the server looking hung.
    tokio::spawn(async move {
        loop {
            match raw_rx.recv().await {
                Ok(notification) if notification.method == "language/status" => {
                    let message = notification
                        .params
                        .get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_owned();
                    if !message.is_empty()
                        && status_updates
                            .send(LspUpdate::ServerStatus {
                                token,
                                key: status_key.clone(),
                                message,
                            })
                            .is_err()
                    {
                        return;
                    }
                },
                Ok(_) => {},
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    tokio::spawn(async move {
        loop {
            match diagnostic_rx.recv().await {
                Ok(publication) => {
                    let _ = updates.send(LspUpdate::Diagnostics {
                        token,
                        server: key.clone(),
                        path: publication.path,
                        version: publication.version,
                        diagnostics: publication.diagnostics,
                    });
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "language-server diagnostic subscriber lagged");
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
