//! The seam contract for language intelligence beyond a plain round trip: a
//! server asking for its hints to be refreshed, and a server that does not
//! offer what it is asked for.
//!
//! Split from `roundtrip_tests`, which proves the columns survive the wire.
//! These prove which *events* a client sees, and in what order.

use super::*;

/// Open `path` and wait for the session to confirm it.
async fn open(
    backend: &impl Backend,
    events: &mut EventRx,
    path: PathBuf,
) -> Result<(DocumentId, u64), Box<dyn std::error::Error + Send + Sync>> {
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    Ok(await_opened(events).await.ok_or("no Opened")?)
}

#[tokio::test]
async fn a_servers_refresh_request_reaches_the_client_as_an_event() -> TestResult {
    // The edit that motivates it: `-> u32` becomes `-> u64` in one file and
    // every `: u32` hint at a call site in another is now wrong. Only the
    // server knows that, and this is how it says so.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "a.rs", "fn a() -> u32 { 0 }\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::RefreshesHints, None, spawns));
    let backend = local_session(session, None);
    let _ = open(&backend, &mut events, path).await?;

    loop {
        let (id, event) = next_event(&mut events)
            .await
            .ok_or("no InlayHintsRefresh event")?;
        if let Event::InlayHintsRefresh { server } = event {
            assert_eq!(id, None, "a refresh answers no client request");
            assert_eq!(server, LanguageServerId::RustAnalyzer);
            return Ok(());
        }
    }
}

/// Insert `text` at the start of line 1 of `doc`, at `base`.
fn insert(
    backend: &impl Backend,
    doc: DocumentId,
    base: u64,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let at = Range::new(LineCol::new(1, 0), LineCol::new(1, 0)).map_err(|e| format!("{e}"))?;
    backend.send(
        backend.next_id(),
        Command::ApplyChange {
            doc,
            change: Change::new(
                base,
                vec![TextEdit {
                    range: at,
                    new_text: text.to_owned(),
                }],
            ),
            cause: EditCause::Type,
        },
    )?;
    Ok(())
}

fn whole_first_line() -> Range {
    Range {
        start: LineCol::new(0, 0),
        end: LineCol::new(0, u32::MAX),
    }
}

#[tokio::test]
async fn a_slow_hint_request_does_not_hold_up_completion() -> TestResult {
    // Hints are background work the editor asks for by itself; completion is
    // something the user is waiting on. Awaiting the first in line put the
    // second behind a server's slowest inference pass -- here, forever.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "let a = 1;\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::HintsNever, None, spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    backend.send(
        backend.next_id(),
        Command::InlayHints {
            doc,
            range: whole_first_line(),
        },
    )?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 3),
        },
    )?;
    let (rid, _, _, items) = await_completions(&mut events)
        .await
        .ok_or("completion waited behind an unanswered hint request")?;
    assert_eq!(rid, Some(request));
    assert_eq!(items.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_hint_request_waits_for_its_edit_without_flushing_it_early() -> TestResult {
    // Two edits and a hint request between them, all inside one debounce
    // window. Flushing for the hint request would send the first edit on its
    // own and then the second; waiting for the debounce sends both as one
    // `didChange`, and the hint request after it -- against the text it was
    // asked about, never the text before it.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "lib.rs", "fn a() {}\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, version) = open(&backend, &mut events, path).await?;

    insert(&backend, doc, version, "x")?;
    backend.send(
        backend.next_id(),
        Command::InlayHints {
            doc,
            range: whole_first_line(),
        },
    )?;
    insert(&backend, doc, version + 1, "y")?;
    let _ = await_inlay_hints(&mut events)
        .await
        .ok_or("the deferred hint request was never answered")?;

    let mut changes = Vec::new();
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), observed_rx.recv())
            .await?
            .ok_or("server stream ended")?;
        match msg["method"].as_str() {
            Some("textDocument/didChange") => {
                changes.push(msg["params"]["contentChanges"][0]["text"].clone());
            },
            Some("textDocument/inlayHint") => break,
            _ => {},
        }
    }
    assert_eq!(
        changes,
        vec![json!("fn a() {}\nyx")],
        "the hint request flushed an edit the debounce was still holding"
    );
    Ok(())
}

#[tokio::test]
async fn a_server_without_inlay_hints_answers_them_empty() -> TestResult {
    // Refused before the wire, and still answered: the editor asks on every
    // viewport change and must never be left holding a request open.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "let a = 1;\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Bare, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, version) = open(&backend, &mut events, path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::InlayHints {
            doc,
            range: Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, u32::MAX),
            },
        },
    )?;
    let (rid, hdoc, hversion, hints) = await_inlay_hints(&mut events)
        .await
        .ok_or("no InlayHints event")?;
    assert_eq!(rid, Some(request));
    assert_eq!((hdoc, hversion), (doc, version));
    assert!(hints.is_empty());

    // And nothing reached the server but the document itself.
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(300), observed_rx.recv()).await
    {
        assert_ne!(
            msg["method"], "textDocument/inlayHint",
            "a refused request reached the wire"
        );
    }
    Ok(())
}
