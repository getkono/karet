//! Round-tripping a request through a live connection, and the column
//! conversion on both legs of it.
//!
//! Split from `tests`, which owns the scaffolding and the lifecycle cases.
//! These are the ones that put a real request on the wire and read a real
//! answer back, so they are also where a column that survives the trip is
//! actually proven.

use super::*;

#[tokio::test]
async fn completion_round_trips_with_utf16_conversion() -> TestResult {
    let dir = tempfile::tempdir()?;
    // '😀' is 1 buffer column but 2 UTF-16 units.
    let path = rust_file(&dir, "main.rs", "😀ab\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);

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
async fn document_symbols_round_trip_with_utf16_conversion() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "😀ab\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(request, Command::DocumentSymbols { doc })?;
    let (answer, answer_doc, symbols) =
        await_symbols(&mut events).await.ok_or("no Symbols event")?;
    assert_eq!(answer, Some(request));
    assert_eq!(answer_doc, doc);
    assert_eq!(symbols[0].selection_range.start, LineCol::new(0, 1));
    assert_eq!(symbols[0].selection_range.end, LineCol::new(0, 3));
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
    let backend = local_session(session, None);

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
