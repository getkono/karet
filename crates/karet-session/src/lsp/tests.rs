use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use karet_core::Change;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::TextEdit;
use karet_text::EditCause;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::DuplexStream;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;

use super::*;
use crate::api::Command;
use crate::api::Event;
use crate::backend::Backend;
use crate::backend::local;
use crate::session::EventRx;
use crate::session::Session;
use crate::session::SessionConfig;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn reconfigure_retires_updates_from_old_server_tasks() {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None);
    let old = LspUpdate::Completions {
        generation: 0,
        request: RequestId(1),
        doc: DocumentId(1),
        version: 1,
        items: Vec::new(),
    };
    assert!(manager.accepts(&old));

    let settings = LspSettings {
        enabled: false,
        ..LspSettings::default()
    };
    assert!(manager.reconfigure(settings.clone()));
    assert!(!manager.accepts(&old));
    assert!(
        !manager.reconfigure(settings),
        "an identical snapshot is a no-op"
    );
}

// --- a minimal LSP wire for the fake server (framing + JSON) -----------

async fn read_msg(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Option<Value> {
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

async fn write_msg(writer: &mut WriteHalf<DuplexStream>, message: &Value) {
    let body = serde_json::to_vec(message).unwrap_or_default();
    let head = format!("Content-Length: {}\r\n\r\n", body.len());
    let _ = writer.write_all(head.as_bytes()).await;
    let _ = writer.write_all(&body).await;
    let _ = writer.flush().await;
}

/// What the scripted server should do after the initialize handshake.
#[derive(Clone, Copy)]
enum Behavior {
    /// Serve completions; echo every received message to `observed`.
    Normal,
    /// Hang up right after the handshake (a crashing server).
    DieAfterHandshake,
}

/// A connector that runs a scripted in-memory server per "spawn".
fn test_connector(
    behavior: Behavior,
    observed: Option<mpsc::UnboundedSender<Value>>,
    spawns: Arc<AtomicUsize>,
) -> Connector {
    Arc::new(move |_spec, root| {
        let observed = observed.clone();
        let spawns = Arc::clone(&spawns);
        Box::pin(async move {
            spawns.fetch_add(1, Ordering::SeqCst);
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
                                "result": {"capabilities": {}}}),
                )
                .await;
                let _initialized = read_msg(&mut reader).await;
                if matches!(behavior, Behavior::DieAfterHandshake) {
                    return; // both halves drop: the client sees EOF
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
fn failing_connector(spawns: Arc<AtomicUsize>) -> Connector {
    Arc::new(move |_spec, _root| {
        spawns.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(LspError::Spawn) })
    })
}

// --- session-level helpers ---------------------------------------------

fn rust_file(dir: &tempfile::TempDir, name: &str, text: &str) -> Option<PathBuf> {
    let path = dir.path().join(name);
    std::fs::write(&path, text).ok()?;
    Some(path)
}

async fn next_event(events: &mut EventRx) -> Option<(Option<RequestId>, Event)> {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .ok()
        .flatten()
}

async fn await_opened(events: &mut EventRx) -> Option<(DocumentId, u64)> {
    while let Some((_, event)) = next_event(events).await {
        if let Event::Opened { doc, version } = event {
            return Some((doc, version));
        }
    }
    None
}

async fn await_completions(
    events: &mut EventRx,
) -> Option<(Option<RequestId>, DocumentId, u64, Vec<CompletionItem>)> {
    while let Some((rid, event)) = next_event(events).await {
        if let Event::Completions {
            doc,
            version,
            items,
        } = event
        {
            return Some((rid, doc, version, items));
        }
    }
    None
}

fn session_with_connector(connector: Connector) -> (Session, EventRx) {
    let (mut session, events, _snaps) = Session::new(SessionConfig::default());
    session.set_lsp_connector(connector);
    (session, events)
}

// --- the tests -----------------------------------------------------------

#[tokio::test]
async fn completion_round_trips_with_utf16_conversion() -> TestResult {
    let dir = tempfile::tempdir()?;
    // '😀' is 1 buffer column but 2 UTF-16 units.
    let path = rust_file(&dir, "main.rs", "😀ab\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local(session);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;

    // Caret after "😀ab" = buffer col 3.
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 3),
        },
    )?;

    let (rid, cdoc, cversion, items) = await_completions(&mut events)
        .await
        .ok_or("no Completions event")?;
    assert_eq!(rid, Some(request), "answer tagged with the request id");
    assert_eq!(cdoc, doc);
    assert_eq!(cversion, version);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "emoji_aware");
    // The server's UTF-16 range 2..4 is buffer cols 1..3 (after the emoji).
    let edit = items[0].edit.clone().ok_or("expected an edit")?;
    assert_eq!(edit.range.start, LineCol::new(0, 1));
    assert_eq!(edit.range.end, LineCol::new(0, 3));

    // And the outgoing request carried the UTF-16 position (col 3 → 4).
    let mut saw_utf16 = false;
    while let Ok(msg) = tokio::time::timeout(Duration::from_secs(5), observed_rx.recv()).await {
        let Some(msg) = msg else { break };
        if msg["method"] == "textDocument/completion" {
            assert_eq!(
                msg["params"]["position"],
                json!({"line": 0, "character": 4})
            );
            saw_utf16 = true;
            break;
        }
    }
    assert!(saw_utf16, "the completion request should reach the server");
    Ok(())
}

#[tokio::test]
async fn open_and_debounced_changes_reach_the_server() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "lib.rs", "fn a() {}\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local(session);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;

    // Two rapid single-char inserts; the debounce coalesces them.
    for (i, ch) in ["x", "y"].iter().enumerate() {
        let range =
            Range::new(LineCol::new(1, 0), LineCol::new(1, 0)).map_err(|e| format!("{e}"))?;
        backend.send(
            backend.next_id(),
            Command::ApplyChange {
                doc,
                change: Change::new(
                    version + i as u64,
                    vec![TextEdit {
                        range,
                        new_text: (*ch).to_owned(),
                    }],
                ),
                cause: EditCause::Type,
            },
        )?;
    }
    // A completion flushes the pending change ahead of itself.
    backend.send(
        backend.next_id(),
        Command::Completion {
            doc,
            position: LineCol::new(1, 1),
        },
    )?;
    let _ = await_completions(&mut events).await.ok_or("no answer")?;

    // Server-side order: didOpen (with the original text), then didChange(s)
    // whose final text is the current buffer, then the completion.
    let mut methods = Vec::new();
    let mut last_change_text = String::new();
    while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_secs(5), observed_rx.recv()).await
    {
        let method = msg["method"].as_str().unwrap_or_default().to_owned();
        if method == "textDocument/didChange" {
            last_change_text = msg["params"]["contentChanges"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
        }
        if method == "textDocument/didOpen" {
            assert_eq!(msg["params"]["textDocument"]["text"], json!("fn a() {}\n"));
            assert_eq!(msg["params"]["textDocument"]["languageId"], json!("rust"));
        }
        let done = method == "textDocument/completion";
        methods.push(method);
        if done {
            break;
        }
    }
    assert_eq!(
        methods.first().map(String::as_str),
        Some("textDocument/didOpen")
    );
    assert_eq!(
        methods.last().map(String::as_str),
        Some("textDocument/completion")
    );
    assert!(
        methods.iter().any(|m| m == "textDocument/didChange"),
        "edits must be forwarded, got {methods:?}"
    );
    // Both single-char inserts are visible in the last forwarded text.
    assert_eq!(last_change_text, "fn a() {}\nyx");
    Ok(())
}

#[tokio::test]
async fn unsupported_language_answers_empty_immediately() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "notes.txt", "plain text\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, Arc::clone(&spawns)));
    let backend = local(session);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;
    let (rid, cdoc, cversion, items) = await_completions(&mut events).await.ok_or("no answer")?;
    assert_eq!(rid, Some(request));
    assert_eq!((cdoc, cversion), (doc, version));
    assert!(items.is_empty());
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "no server for .txt");
    Ok(())
}

#[tokio::test]
async fn disabled_setting_spawns_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let mut config = SessionConfig::default();
    config.settings.lsp.enabled = false;
    let (mut session, mut events, _snaps) = Session::new(config);
    session.set_lsp_connector(test_connector(Behavior::Normal, None, Arc::clone(&spawns)));
    let backend = local(session);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;
    let (rid, _, _, items) = await_completions(&mut events).await.ok_or("no answer")?;
    assert_eq!(rid, Some(request));
    assert!(items.is_empty());
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "disabled means no spawns");
    Ok(())
}

#[tokio::test]
async fn missing_binary_warns_once_and_answers_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let first = rust_file(&dir, "a.rs", "fn a() {}\n").ok_or("write failed")?;
    let second = rust_file(&dir, "b.rs", "fn b() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(failing_connector(Arc::clone(&spawns)));
    let backend = local(session);

    // Two documents of the same language: one spawn attempt, one warning.
    for path in [first, second] {
        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
    }
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;

    // Drain until the completion answer; count LSP warnings seen on the way.
    let mut lsp_warnings = 0;
    let mut answered = false;
    while let Some((rid, event)) = next_event(&mut events).await {
        match event {
            Event::Notification {
                kind: NotificationKind::Lsp,
                ..
            } => lsp_warnings += 1,
            Event::Completions { items, .. } => {
                assert_eq!(rid, Some(request));
                assert!(items.is_empty());
                answered = true;
                break;
            },
            _ => {},
        }
    }
    assert!(answered, "a dead server must still answer completions");
    assert_eq!(lsp_warnings, 1, "exactly one missing-binary warning");
    assert_eq!(spawns.load(Ordering::SeqCst), 1, "one attempt, remembered");
    Ok(())
}

#[tokio::test]
async fn server_death_is_reported_and_completions_stay_answered() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(
        Behavior::DieAfterHandshake,
        None,
        Arc::clone(&spawns),
    ));
    let backend = local(session);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;

    let mut died_notice = false;
    let mut answered = false;
    while let Some((rid, event)) = next_event(&mut events).await {
        match event {
            Event::Notification {
                kind: NotificationKind::Lsp,
                message,
                ..
            } => {
                assert!(message.contains("stopped"), "unexpected: {message}");
                died_notice = true;
                if answered {
                    break;
                }
            },
            Event::Completions { items, .. } => {
                assert_eq!(rid, Some(request));
                assert!(items.is_empty());
                answered = true;
                if died_notice {
                    break;
                }
            },
            _ => {},
        }
    }
    assert!(answered && died_notice);
    Ok(())
}

// --- unit tests for the pure pieces --------------------------------------

#[test]
fn builtin_specs_cover_the_documented_languages() {
    let rust = builtin_spec("rust");
    assert_eq!(rust.map(|s| s.command), Some("rust-analyzer".to_owned()));
    for lang in ["typescript", "javascript"] {
        let spec = builtin_spec(lang);
        assert_eq!(
            spec.as_ref().map(|s| s.command.as_str()),
            Some("typescript-language-server")
        );
        assert_eq!(spec.map(|s| s.args), Some(vec!["--stdio".to_owned()]));
    }
    let py = builtin_spec("python");
    assert_eq!(
        py.map(|s| (s.command, s.args)),
        Some(("pyright-langserver".to_owned(), vec!["--stdio".to_owned()]))
    );
    assert!(builtin_spec("cobol").is_none());
}

#[test]
fn user_config_overrides_builtins() {
    let mut settings = LspSettings::default();
    settings.servers.insert(
        "rust".to_owned(),
        crate::config::schema::LspServer {
            command: "my-ra".to_owned(),
            args: vec!["--custom".to_owned()],
        },
    );
    // And extends to languages with no builtin.
    settings.servers.insert(
        "zig".to_owned(),
        crate::config::schema::LspServer {
            command: "zls".to_owned(),
            args: Vec::new(),
        },
    );
    let (manager, _rx) = LspManager::new(settings, None);
    let rust = manager.spec_for("rust");
    assert_eq!(
        rust.map(|s| (s.command, s.args)),
        Some(("my-ra".to_owned(), vec!["--custom".to_owned()]))
    );
    assert_eq!(
        manager.spec_for("zig").map(|s| s.command),
        Some("zls".to_owned())
    );
    // Untouched languages keep their builtin.
    assert_eq!(
        manager.spec_for("python").map(|s| s.command),
        Some("pyright-langserver".to_owned())
    );
}

#[test]
fn language_keys_lowercase_display_names() {
    assert_eq!(language_key(Some("Rust")), Some("rust".to_owned()));
    assert_eq!(
        language_key(Some("TypeScript")),
        Some("typescript".to_owned())
    );
    assert_eq!(language_key(None), None);
}

#[test]
fn versions_clamp_into_i32() {
    assert_eq!(version_i32(0), 0);
    assert_eq!(version_i32(41), 41);
    assert!(version_i32(u64::MAX) >= 0);
}
