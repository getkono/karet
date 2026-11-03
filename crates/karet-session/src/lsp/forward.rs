//! Relaying what a connected server pushes: diagnostics, the status
//! notifications some servers use in place of them, its requests to refresh
//! what it answered before, and its requests to apply a workspace edit.
//!
//! Split out of `runtime`, which owns the task's own lifecycle. This is the one
//! piece that outlives a single loop iteration -- it runs as its own task for as
//! long as the connection does, and is aborted with it.

use karet_lsp::LspClient;
use tokio::sync::mpsc;

use super::message::DiagnosticFeed;
use super::message::LspUpdate;
use super::slot::SlotKey;
use super::slot::SlotToken;

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
    token: SlotToken,
) -> tokio::task::JoinHandle<()> {
    super::window::forward_window(client, &updates, &key, token);
    let mut diagnostic_rx = client.diagnostics();
    let mut raw_rx = client.raw_notifications();
    let mut refresh_rx = client.refreshes();
    // A server's `workspace/applyEdit` waits on the session, which applies it
    // and answers. Each request carries its own reply, so one the session never
    // sees -- the slot retired, the session gone -- is answered by its `Drop`.
    if let Some(mut edits) = client.apply_edit_requests() {
        let edit_updates = updates.clone();
        let edit_key = key.clone();
        tokio::spawn(async move {
            while let Some(request) = edits.recv().await {
                let update = LspUpdate::ApplyEdit {
                    token,
                    key: edit_key.clone(),
                    request,
                };
                if edit_updates.send(update).is_err() {
                    return;
                }
            }
        });
    }
    let status_updates = updates.clone();
    let status_key = key.clone();
    let refresh_updates = updates.clone();
    let refresh_key = key.clone();
    // The server has already been answered; what is relayed is the other half
    // of the promise `refreshSupport` made -- that the client re-asks.
    tokio::spawn(async move {
        loop {
            match refresh_rx.recv().await {
                Ok(karet_lsp::ServerRefresh::InlayHints)
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // A lag lost only duplicates of the one signal there is,
                    // so it is relayed as that signal rather than dropped.
                    if refresh_updates
                        .send(LspUpdate::InlayHintsRefresh {
                            token,
                            key: refresh_key.clone(),
                        })
                        .is_err()
                    {
                        return;
                    }
                },
                Ok(_) => {},
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    // jdtls-style `language/status` notifications carry the only feedback a
    // user gets during a 30–120 s first import; forward them for the status
    // bar rather than leaving the server looking hung. Copilot's
    // `didChangeStatus` likewise, when it reports a problem: it is how the
    // user learns ghost text needs a sign-in.
    tokio::spawn(async move {
        loop {
            match raw_rx.recv().await {
                Ok(notification) => {
                    let Some(message) = status_message(&notification.method, &notification.params)
                    else {
                        continue;
                    };
                    let update = LspUpdate::ServerStatus {
                        token,
                        key: status_key.clone(),
                        message,
                    };
                    if status_updates.send(update).is_err() {
                        return;
                    }
                },
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
                        feed: DiagnosticFeed::Pushed,
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

/// The status line a pushed notification carries, when it is one worth
/// showing: a non-empty `language/status` message, or a `didChangeStatus`
/// (Copilot) message reporting an `Error` or `Warning` -- its `Normal` and
/// `Inactive` reports, sent on every keystroke's busy flip, would only be
/// noise.
fn status_message(method: &str, params: &serde_json::Value) -> Option<String> {
    let message = params.get("message")?.as_str()?;
    let worth = match method {
        "language/status" => true,
        "didChangeStatus" => matches!(
            params.get("kind").and_then(serde_json::Value::as_str),
            Some("Error" | "Warning")
        ),
        _ => false,
    };
    (worth && !message.is_empty()).then(|| message.to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_status_lines_worth_showing_are_forwarded() {
        let language = json!({"message": "Importing project"});
        assert_eq!(
            status_message("language/status", &language).as_deref(),
            Some("Importing project")
        );
        assert_eq!(
            status_message("language/status", &json!({"message": ""})),
            None
        );
        let signed_out =
            json!({"kind": "Error", "message": "You are not signed in", "busy": false});
        assert_eq!(
            status_message("didChangeStatus", &signed_out).as_deref(),
            Some("You are not signed in")
        );
        let normal = json!({"kind": "Normal", "message": "", "busy": true});
        assert_eq!(status_message("didChangeStatus", &normal), None);
        let inactive = json!({"kind": "Inactive", "message": "file excluded"});
        assert_eq!(status_message("didChangeStatus", &inactive), None);
        assert_eq!(status_message("$/progress", &language), None);
    }
}
