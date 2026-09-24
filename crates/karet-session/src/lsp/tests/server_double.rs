//! The scripted in-memory language server the session tests run against.
//!
//! A real process is the one thing these tests must not need, and a trait double
//! would prove nothing: what is under test is the wire — framing, the handshake,
//! the capabilities a server advertises and the replies it sends. So this speaks
//! actual LSP over a `duplex` pipe, and [`Behavior`] scripts what it does once
//! the handshake is done.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use karet_lsp::LspClient;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::DuplexStream;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;
use tokio::sync::mpsc;

use super::super::Connector;
use super::super::LspError;

/// Read one `Content-Length`-framed message, for a test that speaks the wire
/// itself rather than scripting this double.
pub(super) async fn read_msg(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Option<Value> {
    let mut len: Option<usize> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await.ok()? == 0 {
            return None;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end();
        if text.is_empty() {
            break;
        }
        if let Some(value) = text.strip_prefix("Content-Length:") {
            len = value.trim().parse().ok();
        }
    }
    let mut body = vec![0_u8; len?];
    reader.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

/// Frame and send one message, the counterpart to [`read_msg`].
pub(super) async fn write_msg(writer: &mut WriteHalf<DuplexStream>, message: &Value) {
    let body = serde_json::to_vec(message).unwrap_or_default();
    let head = format!("Content-Length: {}\r\n\r\n", body.len());
    let _ = writer.write_all(head.as_bytes()).await;
    let _ = writer.write_all(&body).await;
    let _ = writer.flush().await;
}

/// What the scripted server should do after the initialize handshake.
#[derive(Clone, Copy)]
pub(super) enum Behavior {
    /// Serve completions; echo every received message to `observed`.
    Normal,
    /// Hang up right after the handshake (a crashing server).
    DieAfterHandshake,
    /// Crash the first process, then serve normally after the supervisor retries.
    DieOnce,
    /// Accept the document, then hang up with nothing outstanding.
    ///
    /// The only way to notice this death is to watch the connection: no request
    /// is ever issued over it, and `didOpen` is a notification, so nothing the
    /// client sends can come back failed.
    DieWhenIdle,
    /// Publish one diagnostic for the opened document, then hang up for good.
    DieAfterDiagnostics,
    /// Advertise `textDocument/formatting` and answer it, replacing the first
    /// line with `formatted\n`. The only behaviour that lets a save actually
    /// reach `begin_format_on_save` and park on a reply.
    Formats,
    /// Advertise `textDocument/formatting` and then never answer it, while
    /// staying perfectly alive and serving everything else. A formatter wedged
    /// on one request is not a dead server, and must not be treated as one.
    FormatsNever,
    /// Serve normally, but never answer `textDocument/inlayHint`: a server
    /// slow to infer, which must not hold up anything else asked of it.
    HintsNever,
    /// Serve normally, but hang up on receiving `textDocument/inlayHint`: the
    /// connection is lost with that request in flight.
    DiesOnHint,
    /// Serve normally, and ask the client to refresh its inlay hints
    /// (`workspace/inlayHint/refresh`) once it has opened a document.
    RefreshesHints,
    /// Advertise nothing but document sync, so every gated request is
    /// refused before it reaches the wire.
    Bare,
}

/// What the scripted server advertises at the handshake.
///
/// It has to advertise something: karet refuses a request the server never
/// said it could answer, so a double that advertised `{}` and answered anyway
/// would have every request refused before reaching it. That gap is the point
/// of the gate; see issue #279.
///
/// `textDocument/formatting` is the one exception, advertised only by the
/// formatting behaviours: whether a server formats decides between it and the
/// built-in formatter at save, and the tests of that choice need a server that
/// does not.
fn advertised_capabilities(behavior: Behavior) -> Value {
    if matches!(behavior, Behavior::Bare) {
        return json!({"textDocumentSync": 1});
    }
    let mut capabilities = json!({
        "textDocumentSync": 1,
        "hoverProvider": true,
        "completionProvider": {"resolveProvider": true},
        "definitionProvider": true,
        "documentSymbolProvider": true,
        "workspaceSymbolProvider": true,
        "renameProvider": true,
        "documentRangeFormattingProvider": true,
        "codeActionProvider": true,
        "signatureHelpProvider": {},
        "inlayHintProvider": true,
        "implementationProvider": true,
        "typeHierarchyProvider": true,
    });
    if matches!(behavior, Behavior::Formats | Behavior::FormatsNever) {
        capabilities["documentFormattingProvider"] = json!(true);
    }
    capabilities
}

/// A connector that runs a scripted in-memory server per "spawn".
pub(super) fn test_connector(
    behavior: Behavior,
    observed: Option<mpsc::UnboundedSender<Value>>,
    spawns: Arc<AtomicUsize>,
) -> Connector {
    Arc::new(move |_spec, root| {
        let observed = observed.clone();
        let spawns = Arc::clone(&spawns);
        Box::pin(async move {
            let attempt = spawns.fetch_add(1, Ordering::SeqCst);
            let behavior = if matches!(behavior, Behavior::DieOnce) && attempt > 0 {
                Behavior::Normal
            } else {
                behavior
            };
            let (client_end, server_end) = tokio::io::duplex(1 << 20);
            let (server_read, mut server_write) = tokio::io::split(server_end);
            tokio::spawn(async move {
                let mut reader = BufReader::new(server_read);
                // Handshake.
                let Some(init) = read_msg(&mut reader).await else {
                    return;
                };
                write_msg(
                    &mut server_write,
                    &json!({"jsonrpc": "2.0", "id": init["id"],
                                "result": {"capabilities": advertised_capabilities(behavior)}}),
                )
                .await;
                let _initialized = read_msg(&mut reader).await;
                if matches!(behavior, Behavior::DieAfterHandshake | Behavior::DieOnce) {
                    return; // both halves drop: the client sees EOF
                }
                if matches!(
                    behavior,
                    Behavior::DieWhenIdle | Behavior::DieAfterDiagnostics
                ) {
                    let open = read_msg(&mut reader).await;
                    if matches!(behavior, Behavior::DieAfterDiagnostics)
                        && let Some(open) = open
                        && let Some(uri) = open["params"]["textDocument"]["uri"].as_str()
                    {
                        write_msg(
                            &mut server_write,
                            &json!({"jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": {"uri": uri, "diagnostics": [{
                                "range": {
                                    "start": {"line": 0, "character": 0},
                                    "end": {"line": 0, "character": 2}
                                },
                                "severity": 1,
                                "message": "a marker that must not outlive its server"
                            }]}}),
                        )
                        .await;
                    }
                    return;
                }
                while let Some(msg) = read_msg(&mut reader).await {
                    if let Some(tx) = &observed {
                        let _ = tx.send(msg.clone());
                    }
                    match msg["method"].as_str() {
                        Some("textDocument/completion") => {
                            // A fixed item whose textEdit range is in UTF-16:
                            // chars 2..4 on the requested line.
                            let line = msg["params"]["position"]["line"].clone();
                            write_msg(
                                &mut server_write,
                                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": [{
                                    "label": "emoji_aware",
                                    "kind": 5,
                                    "textEdit": {
                                        "range": {
                                            "start": {"line": line, "character": 2},
                                            "end": {"line": line, "character": 4}
                                        },
                                        "newText": "emoji_aware"
                                    }
                                }]}),
                            )
                            .await;
                        },
                        Some("textDocument/inlayHint")
                            if matches!(behavior, Behavior::HintsNever) => {},
                        Some("textDocument/inlayHint")
                            if matches!(behavior, Behavior::DiesOnHint) =>
                        {
                            break; // both halves drop: the client sees EOF
                        },
                        Some("textDocument/inlayHint") => {
                            // One hint at UTF-16 character 4 on line 0, which
                            // is buffer column 3 once the emoji is accounted
                            // for.
                            write_msg(
                                &mut server_write,
                                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": [{
                                    "position": {"line": 0, "character": 4},
                                    "label": ": i32",
                                    "kind": 1,
                                    "paddingLeft": false,
                                    "paddingRight": false
                                }]}),
                            )
                            .await;
                        },
                        // Nothing to say, which is still an answer.
                        Some("textDocument/hover") => {
                            write_msg(
                                &mut server_write,
                                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": null}),
                            )
                            .await;
                        },
                        Some("textDocument/documentSymbol") => {
                            write_msg(
                                &mut server_write,
                                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": [{
                                    "name": "emoji_name",
                                    "kind": 12,
                                    "range": {
                                        "start": {"line": 0, "character": 0},
                                        "end": {"line": 0, "character": 4}
                                    },
                                    "selectionRange": {
                                        "start": {"line": 0, "character": 2},
                                        "end": {"line": 0, "character": 4}
                                    }
                                }]}),
                            )
                            .await;
                        },
                        Some("textDocument/didOpen") => {
                            let uri = msg["params"]["textDocument"]["uri"]
                                .as_str()
                                .unwrap_or_default();
                            if matches!(behavior, Behavior::RefreshesHints) {
                                write_msg(
                                    &mut server_write,
                                    &json!({"jsonrpc": "2.0", "id": "refresh-1",
                                        "method": "workspace/inlayHint/refresh"}),
                                )
                                .await;
                            }
                            if uri.ends_with("Status.java") {
                                write_msg(
                                    &mut server_write,
                                    &json!({"jsonrpc": "2.0", "method": "language/status",
                                        "params": {"type": "Starting",
                                            "message": "37% Importing projects"}}),
                                )
                                .await;
                            }
                        },
                        // Swallowed whole: no reply, no error, no hang-up.
                        Some("textDocument/formatting")
                            if matches!(behavior, Behavior::FormatsNever) => {},
                        Some("textDocument/formatting") => {
                            write_msg(
                                &mut server_write,
                                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": [{
                                    "range": {
                                        "start": {"line": 0, "character": 0},
                                        "end": {"line": 1, "character": 0}
                                    },
                                    "newText": "formatted\n"
                                }]}),
                            )
                            .await;
                        },
                        Some("shutdown") => {
                            write_msg(
                                &mut server_write,
                                &json!({"jsonrpc": "2.0", "id": msg["id"], "result": null}),
                            )
                            .await;
                        },
                        Some("exit") => break,
                        _ => {},
                    }
                }
            });
            let (read, write) = tokio::io::split(client_end);
            LspClient::connect(read, write, &root).await
        })
    })
}

/// A connector that always fails as if the binary were missing.
pub(super) fn failing_connector(spawns: Arc<AtomicUsize>) -> Connector {
    Arc::new(move |spec, _root| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let failure = karet_lsp::LaunchFailure::new(
            spec.command.clone(),
            spec.args.clone(),
            karet_lsp::LaunchCause::NotFound,
        );
        Box::pin(async move { Err(LspError::Launch(Box::new(failure))) })
    })
}
