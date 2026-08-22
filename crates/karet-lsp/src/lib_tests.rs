use std::time::Duration;

use karet_core::Severity;
use serde_json::json;
use tokio::io::DuplexStream;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// The scripted fake server side of an in-memory connection.
struct FakeServer {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

/// An in-memory wire: the client's `(read, write)` halves plus the fake
/// server holding the other end.
fn wire() -> (
    (ReadHalf<DuplexStream>, WriteHalf<DuplexStream>),
    FakeServer,
) {
    let (client_end, server_end) = tokio::io::duplex(1 << 20);
    let (client_read, client_write) = tokio::io::split(client_end);
    let (server_read, server_write) = tokio::io::split(server_end);
    (
        (client_read, client_write),
        FakeServer {
            reader: BufReader::new(server_read),
            writer: server_write,
        },
    )
}

impl FakeServer {
    /// Read one message, or `Null` on EOF/parse failure.
    async fn recv(&mut self) -> Value {
        match codec::read_frame(&mut self.reader).await {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    async fn send(&mut self, message: &Value) {
        let bytes = serde_json::to_vec(message).unwrap_or_default();
        let _ = codec::write_frame(&mut self.writer, &bytes).await;
    }

    async fn respond(&mut self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
    }

    /// Serve the `initialize`/`initialized` handshake, returning the
    /// `initialize` params for assertions.
    async fn handshake(&mut self) -> Value {
        let init = self.recv().await;
        assert_eq!(init["method"], "initialize");
        let id = init["id"].clone();
        self.respond(&id, json!({"capabilities": {}})).await;
        let initialized = self.recv().await;
        assert_eq!(initialized["method"], "initialized");
        init["params"].clone()
    }
}

#[tokio::test]
async fn connect_negotiates_utf16_and_no_snippets() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        let params = server.handshake().await;
        assert_eq!(
            params["capabilities"]["general"]["positionEncodings"],
            json!(["utf-16"])
        );
        assert_eq!(
            params["capabilities"]["textDocument"]["completion"]["completionItem"]["snippetSupport"],
            json!(false)
        );
        assert_eq!(params["rootUri"], json!("file:///tmp/ws"));
        assert_eq!(
            params["workspaceFolders"][0]["uri"],
            json!("file:///tmp/ws")
        );
        assert_eq!(params["workspaceFolders"][0]["name"], json!("ws"));
    });
    let _client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn document_sync_notifications_reach_the_server() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;

        let open = server.recv().await;
        assert_eq!(open["method"], "textDocument/didOpen");
        let doc = &open["params"]["textDocument"];
        assert_eq!(doc["uri"], json!("file:///tmp/my%20ws/main.rs"));
        assert_eq!(doc["languageId"], json!("rust"));
        assert_eq!(doc["version"], json!(0));
        assert_eq!(doc["text"], json!("fn main() {}\n"));

        let change = server.recv().await;
        assert_eq!(change["method"], "textDocument/didChange");
        assert_eq!(change["params"]["textDocument"]["version"], json!(1));
        let changes = &change["params"]["contentChanges"];
        assert_eq!(changes.as_array().map(Vec::len), Some(1));
        // Full-text sync: the event carries only `text`, never a range.
        assert_eq!(changes[0], json!({"text": "fn main() { }\n"}));

        let save = server.recv().await;
        assert_eq!(save["method"], "textDocument/didSave");
        assert_eq!(save["params"]["text"], json!("fn main() { }\n"));

        let close = server.recv().await;
        assert_eq!(close["method"], "textDocument/didClose");
        assert_eq!(
            close["params"]["textDocument"]["uri"],
            json!("file:///tmp/my%20ws/main.rs")
        );
    });

    let client = LspClient::connect(read, write, Path::new("/tmp/my ws")).await?;
    let doc = Path::new("/tmp/my ws/main.rs");
    client.did_open(doc, "rust", 0, "fn main() {}\n").await?;
    client.did_change(doc, 1, "fn main() { }\n").await?;
    client.did_save(doc, Some("fn main() { }\n")).await?;
    client.did_close(doc).await?;
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn server_initiated_requests_are_answered() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;

        server
            .send(&json!({
                "jsonrpc": "2.0", "id": 100, "method": "workspace/configuration",
                "params": {"items": [{"section": "rust"}, {"section": "fmt"}]}
            }))
            .await;
        let answer = server.recv().await;
        assert_eq!(answer["id"], json!(100));
        assert_eq!(answer["result"], json!([null, null]));

        // String ids must be echoed verbatim.
        server
            .send(&json!({
                "jsonrpc": "2.0", "id": "reg-1", "method": "client/registerCapability",
                "params": {"registrations": []}
            }))
            .await;
        let answer = server.recv().await;
        assert_eq!(answer["id"], json!("reg-1"));
        assert_eq!(answer["result"], json!(null));

        server
            .send(&json!({
                "jsonrpc": "2.0", "id": 101, "method": "window/workDoneProgress/create",
                "params": {"token": "t"}
            }))
            .await;
        let answer = server.recv().await;
        assert_eq!(answer["result"], json!(null));

        server
            .send(&json!({
                "jsonrpc": "2.0", "id": 102, "method": "window/showMessageRequest",
                "params": {"type": 1, "message": "hi"}
            }))
            .await;
        let answer = server.recv().await;
        assert_eq!(answer["id"], json!(102));
        assert_eq!(answer["error"]["code"], json!(-32601));
    });

    let _client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn published_diagnostics_are_broadcast_and_mapped() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server
            .send(&json!({
                "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": "file:///tmp/ws/a.rs",
                    "version": 7,
                    "diagnostics": [{
                        "range": {"start": {"line": 2, "character": 4},
                                  "end": {"line": 2, "character": 9}},
                        "severity": 2,
                        "message": "unused variable",
                        "source": "rustc",
                        "tags": [1]
                    }]
                }
            }))
            .await;
        server // an unknown notification must be ignored without breaking the stream
            .send(&json!({"jsonrpc": "2.0", "method": "window/logMessage",
                          "params": {"type": 3, "message": "noise"}}))
            .await;
    });

    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let mut rx = client.diagnostics();
    let publication = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await??;
    assert_eq!(publication.path, PathBuf::from("/tmp/ws/a.rs"));
    assert_eq!(publication.version, Some(7));
    let diags = publication.diagnostics;
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Severity::Warning);
    assert_eq!(diags[0].message, "unused variable");
    assert_eq!(diags[0].range.start, LineCol::new(2, 4));
    assert_eq!(diags[0].range.end, LineCol::new(2, 9));
    assert_eq!(diags[0].tags, vec![karet_core::DiagnosticTag::Unnecessary]);
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn responses_correlate_out_of_order() -> TestResult {
    let ((read, write), mut server) = wire();
    let connection = conn::Connection::start(read, write);
    let server_task = tokio::spawn(async move {
        let first = server.recv().await;
        let second = server.recv().await;
        assert_eq!(first["method"], "test/one");
        assert_eq!(second["method"], "test/two");
        // Answer in reverse order.
        let second_id = second["id"].clone();
        let first_id = first["id"].clone();
        server.respond(&second_id, json!("two")).await;
        server.respond(&first_id, json!("one")).await;
    });

    let (one, two) = tokio::join!(
        connection.request::<_, String>("test/one", Value::Null),
        connection.request::<_, String>("test/two", Value::Null),
    );
    assert_eq!(one?, "one");
    assert_eq!(two?, "two");
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn error_responses_map_to_server_errors() -> TestResult {
    let ((read, write), mut server) = wire();
    let connection = conn::Connection::start(read, write);
    let server_task = tokio::spawn(async move {
        let req = server.recv().await;
        server
            .send(&json!({"jsonrpc": "2.0", "id": req["id"],
                          "error": {"code": -32000, "message": "boom"}}))
            .await;
    });
    let err = connection
        .request::<_, Value>("test/fails", Value::Null)
        .await;
    let Err(LspError::Server(message)) = err else {
        return Err("expected a server error".into());
    };
    assert!(message.contains("boom"), "unexpected message: {message}");
    assert!(message.contains("-32000"), "unexpected message: {message}");
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn unanswered_requests_time_out() -> TestResult {
    let ((read, write), server) = wire();
    let connection = conn::Connection::start(read, write);
    // Keep the server end alive but silent, so the failure is a timeout,
    // not a closed connection.
    let err = connection
        .request_with::<_, Value>("test/silence", Value::Null, Duration::from_millis(50))
        .await;
    assert!(matches!(err, Err(LspError::Timeout)));
    drop(server);
    Ok(())
}

#[tokio::test]
async fn eof_fails_in_flight_requests_with_closed() -> TestResult {
    let ((read, write), mut server) = wire();
    let connection = conn::Connection::start(read, write);
    let server_task = tokio::spawn(async move {
        let req = server.recv().await;
        assert_eq!(req["method"], "test/doomed");
        drop(server); // hang up without answering
    });
    let err = connection
        .request::<_, Value>("test/doomed", Value::Null)
        .await;
    assert!(matches!(err, Err(LspError::Closed)));
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn requests_after_eof_fail_fast_with_closed() -> TestResult {
    let ((read, write), server) = wire();
    let connection = conn::Connection::start(read, write);
    drop(server); // the server is gone before any request is issued
    // Give the reader task a chance to observe the EOF.
    tokio::task::yield_now().await;
    // A generous deadline proves we do NOT wait it out: the request must
    // fail promptly with Closed, not eventually with Timeout.
    let started = std::time::Instant::now();
    let err = connection
        .request_with::<_, Value>("test/late", Value::Null, Duration::from_secs(30))
        .await;
    assert!(matches!(err, Err(LspError::Closed)), "got {err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn malformed_json_frames_are_skipped() -> TestResult {
    let ((read, write), mut server) = wire();
    let connection = conn::Connection::start(read, write);
    let server_task = tokio::spawn(async move {
        let req = server.recv().await;
        // A well-framed but non-JSON body must not kill the connection …
        let _ = codec::write_frame(&mut server.writer, b"this is not json").await;
        // … nor a JSON body with no JSON-RPC shape.
        server.send(&json!(["still", "not", "jsonrpc"])).await;
        let id = req["id"].clone();
        server.respond(&id, json!("survived")).await;
    });
    let result: String = connection.request("test/resilient", Value::Null).await?;
    assert_eq!(result, "survived");
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn completion_end_to_end() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;

        // 1: a CompletionList (isIncomplete flattened) with a snippet edit.
        let req = server.recv().await;
        assert_eq!(req["method"], "textDocument/completion");
        assert_eq!(
            req["params"]["textDocument"]["uri"],
            json!("file:///tmp/ws/a.rs")
        );
        // UTF-16 position passthrough.
        assert_eq!(
            req["params"]["position"],
            json!({"line": 3, "character": 7})
        );
        let id = req["id"].clone();
        server
            .respond(
                &id,
                json!({
                    "isIncomplete": true,
                    "items": [
                        {
                            "label": "push",
                            "kind": 2,
                            "detail": "fn push(&mut self, ch: char)",
                            "sortText": "0000",
                            "insertTextFormat": 2,
                            "textEdit": {
                                "range": {"start": {"line": 3, "character": 5},
                                          "end": {"line": 3, "character": 7}},
                                "newText": "push(${1:ch})$0"
                            },
                            "tags": [1]
                        },
                        {"label": "plain"}
                    ]
                }),
            )
            .await;

        // 2: a bare array response.
        let req = server.recv().await;
        let id = req["id"].clone();
        server.respond(&id, json!([{"label": "sole"}])).await;

        // 3: a null response (no completions).
        let req = server.recv().await;
        let id = req["id"].clone();
        server.respond(&id, Value::Null).await;
    });

    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let doc = Path::new("/tmp/ws/a.rs");

    let items = client.completion(doc, LineCol::new(3, 7)).await?;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].label, "push");
    assert_eq!(items[0].kind, karet_core::CompletionKind::Method);
    assert_eq!(
        items[0].detail.as_deref(),
        Some("fn push(&mut self, ch: char)")
    );
    assert_eq!(items[0].sort_text.as_deref(), Some("0000"));
    assert_eq!(items[0].insert_text, "push(ch)"); // snippet degraded
    assert!(items[0].deprecated); // via tag
    let edit = items[0].edit.clone().ok_or("expected a text edit")?;
    assert_eq!(edit.range.start, LineCol::new(3, 5));
    assert_eq!(edit.range.end, LineCol::new(3, 7));
    assert_eq!(edit.new_text, "push(ch)");
    assert_eq!(items[1].label, "plain");
    assert_eq!(items[1].insert_text, "plain"); // label fallback
    assert_eq!(items[1].kind, karet_core::CompletionKind::Text);

    let items = client.completion(doc, LineCol::new(0, 0)).await?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "sole");

    let items = client.completion(doc, LineCol::new(0, 0)).await?;
    assert!(items.is_empty());

    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn document_symbols_end_to_end() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let request = server.recv().await;
        assert_eq!(request["method"], "textDocument/documentSymbol");
        assert_eq!(
            request["params"]["textDocument"]["uri"],
            json!("file:///tmp/ws/lib.rs")
        );
        server
            .respond(
                &request["id"],
                json!([{
                    "name": "Runner",
                    "kind": 23,
                    "range": {"start": {"line": 0, "character": 0},
                              "end": {"line": 4, "character": 1}},
                    "selectionRange": {"start": {"line": 0, "character": 7},
                                       "end": {"line": 0, "character": 13}},
                    "children": [{
                        "name": "run",
                        "detail": "(&self)",
                        "kind": 6,
                        "range": {"start": {"line": 2, "character": 2},
                                  "end": {"line": 3, "character": 3}},
                        "selectionRange": {"start": {"line": 2, "character": 5},
                                           "end": {"line": 2, "character": 8}}
                    }]
                }]),
            )
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let symbols = client.document_symbols(Path::new("/tmp/ws/lib.rs")).await?;
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].kind, karet_core::SymbolKind::Struct);
    assert_eq!(symbols[0].children[0].name, "run");
    assert_eq!(symbols[0].children[0].detail.as_deref(), Some("(&self)"));
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn implementations_end_to_end() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let request = server.recv().await;
        assert_eq!(request["method"], "textDocument/implementation");
        server
            .respond(
                &request["id"],
                json!([{
                    "uri": "file:///tmp/ws/impls.rs",
                    "range": {"start": {"line": 7, "character": 0},
                              "end": {"line": 9, "character": 1}}
                }]),
            )
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let found = client
        .implementations(Path::new("/tmp/ws/lib.rs"), LineCol::new(2, 6))
        .await?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, Path::new("/tmp/ws/impls.rs"));
    assert_eq!(found[0].range.start, LineCol::new(7, 0));
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn supertypes_takes_two_round_trips_and_keeps_selection_ranges() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let prepare = server.recv().await;
        assert_eq!(prepare["method"], "textDocument/prepareTypeHierarchy");
        server
            .respond(
                &prepare["id"],
                json!([{
                    "name": "Widget", "kind": 23, "uri": "file:///tmp/ws/lib.rs",
                    "range": {"start": {"line": 0, "character": 0},
                              "end": {"line": 5, "character": 1}},
                    "selectionRange": {"start": {"line": 0, "character": 7},
                                       "end": {"line": 0, "character": 13}}
                }]),
            )
            .await;
        let supertypes = server.recv().await;
        assert_eq!(supertypes["method"], "typeHierarchy/supertypes");
        server
            .respond(
                &supertypes["id"],
                json!([{
                    "name": "Render", "kind": 11, "uri": "file:///tmp/ws/render.rs",
                    "range": {"start": {"line": 1, "character": 0},
                              "end": {"line": 4, "character": 1}},
                    "selectionRange": {"start": {"line": 1, "character": 6},
                                       "end": {"line": 1, "character": 12}}
                }]),
            )
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let found = client
        .supertypes(Path::new("/tmp/ws/lib.rs"), LineCol::new(0, 7))
        .await?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, Path::new("/tmp/ws/render.rs"));
    // The selection range is the navigable one, not the whole body.
    assert_eq!(found[0].range.start, LineCol::new(1, 6));
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn a_server_that_declines_type_hierarchy_yields_no_supertypes() -> TestResult {
    // Not supporting the request is not a failure — the caller degrades to its
    // structural answer rather than surfacing an error.
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let prepare = server.recv().await;
        assert_eq!(prepare["method"], "textDocument/prepareTypeHierarchy");
        server.respond(&prepare["id"], json!(null)).await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let found = client
        .supertypes(Path::new("/tmp/ws/lib.rs"), LineCol::new(0, 7))
        .await?;
    assert!(found.is_empty());
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_performs_the_handshake() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let shutdown = server.recv().await;
        assert_eq!(shutdown["method"], "shutdown");
        let id = shutdown["id"].clone();
        server.respond(&id, Value::Null).await;
        let exit = server.recv().await;
        assert_eq!(exit["method"], "exit");
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    client.shutdown().await?;
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn spawn_missing_binary_is_a_spawn_error() {
    let spec = LspSpec {
        command: "karet-lsp-test-no-such-binary".into(),
        args: vec![],
        languages: vec!["rust".into()],
    };
    let err = LspClient::spawn(spec, Path::new("/tmp")).await;
    assert!(matches!(err, Err(LspError::Spawn)));
}

#[test]
fn error_displays() {
    assert_eq!(LspError::Timeout.to_string(), "request timed out");
    assert_eq!(
        LspError::Closed.to_string(),
        "connection to the language server closed"
    );
    assert_eq!(
        LspError::Protocol("bad frame".into()).to_string(),
        "protocol error: bad frame"
    );
}

#[tokio::test]
async fn custom_request_round_trips_an_untyped_method() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let req = server.recv().await;
        assert_eq!(req["method"], "java/classFileContents");
        assert_eq!(req["params"]["uri"], json!("jdt://contents/rt.jar"));
        let id = req["id"].clone();
        server.respond(&id, json!("class Object {}")).await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let contents: String = client
        .custom_request(
            "java/classFileContents",
            json!({"uri": "jdt://contents/rt.jar"}),
        )
        .await?;
    assert_eq!(contents, "class Object {}");
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn custom_notify_reaches_the_server_verbatim() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let note = server.recv().await;
        assert_eq!(note["method"], "workspace/didChangeConfiguration");
        assert_eq!(
            note["params"]["settings"]["java"]["home"],
            json!("/opt/jdk")
        );
        assert!(note.get("id").is_none(), "a notification carries no id");
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    client.custom_notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"java": {"home": "/opt/jdk"}}}),
    )?;
    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn raw_notifications_fan_out_every_server_notification() -> TestResult {
    let ((read, write), mut server) = wire();
    // Handshake concurrently so the subscription exists before anything is
    // sent — a broadcast with no subscriber drops, which is not under test.
    let (client, ()) = tokio::join!(
        LspClient::connect(read, write, Path::new("/tmp/ws")),
        async {
            server.handshake().await;
        }
    );
    let client = client?;
    let mut raw = client.raw_notifications();
    server
        .send(&json!({
            "jsonrpc": "2.0",
            "method": "language/status",
            "params": {"type": "Started", "message": "Ready"}
        }))
        .await;
    // The diagnostics the typed path consumes still appear on the raw stream.
    server
        .send(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"uri": "file:///tmp/ws/a.rs", "diagnostics": []}
        }))
        .await;
    let first = tokio::time::timeout(Duration::from_secs(5), raw.recv()).await??;
    assert_eq!(first.method, "language/status");
    assert_eq!(first.params["message"], json!("Ready"));
    let second = tokio::time::timeout(Duration::from_secs(5), raw.recv()).await??;
    assert_eq!(second.method, "textDocument/publishDiagnostics");
    Ok(())
}
