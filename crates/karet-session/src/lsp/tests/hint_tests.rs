//! Every inlay-hint request is answered exactly once, whatever becomes of the
//! server it was sent to.
//!
//! A hint request is not awaited in line (see `hint_flight`), so it can be
//! outstanding when its server stops serving: retired by a restart or by the
//! last document closing. Each of those endings must still produce an answer,
//! or the asker waits on a reply that is never coming.

use super::*;

/// Open `path`, wait for the session to confirm it, and wait until the server
/// has the document -- so a hint request sent next is launched, not deferred.
async fn open_served(
    backend: &impl Backend,
    events: &mut EventRx,
    observed: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    path: PathBuf,
) -> Result<DocumentId, Box<dyn std::error::Error + Send + Sync>> {
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(events).await.ok_or("no Opened")?;
    await_method(observed, "textDocument/didOpen").await?;
    Ok(doc)
}

/// Wait until the server has received a message with `method`.
async fn await_method(
    observed: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    method: &str,
) -> TestResult {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await?
            .ok_or("server stream ended")?;
        if message["method"] == method {
            return Ok(());
        }
    }
}

fn whole_first_line() -> Range {
    Range {
        start: LineCol::new(0, 0),
        end: LineCol::new(0, u32::MAX),
    }
}

/// Wait for the answer to `request`, skipping answers to anything else.
async fn await_hint_answer(
    events: &mut EventRx,
    request: RequestId,
) -> Option<Vec<karet_core::InlayHint>> {
    loop {
        let (id, _, _, hints) = await_inlay_hints(events).await?;
        if id == Some(request) {
            return Some(hints);
        }
    }
}

/// A restart retires the slot while a hint request is running on it; the
/// request is answered empty rather than dropped.
///
/// Falsified by: `HintFlight::shutdown` clearing its bookkeeping without
/// answering, as it once did -- the answer never arrives.
#[tokio::test]
async fn a_restart_answers_a_running_hint_request_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "let a = 1;\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(
        Behavior::HintsNever,
        Some(observed_tx),
        spawns,
    ));
    let backend = local_session(session, None);
    let doc = open_served(&backend, &mut events, &mut observed, path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::InlayHints {
            doc,
            range: whole_first_line(),
        },
    )?;
    await_method(&mut observed, "textDocument/inlayHint").await?;

    backend.send(
        backend.next_id(),
        Command::RestartLanguageServer {
            server: LanguageServerId::RustAnalyzer,
        },
    )?;
    let hints = await_hint_answer(&mut events, request)
        .await
        .ok_or("the restart left the hint request unanswered")?;
    assert!(hints.is_empty());
    Ok(())
}

/// The last document closing retires the slot too, and its running hint
/// request is answered on the way out.
///
/// Asserted at the manager rather than the `Command`/`Event` seam, because the
/// session rightly discards an answer for a document that has closed: the
/// contract under test is the manager's, that every request it accepted is
/// answered.
#[tokio::test]
async fn closing_the_last_document_answers_a_running_hint_request() -> TestResult {
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::HintsNever,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/hinted.rs");
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || "let a = 1;".into());
    await_method(&mut observed, "textDocument/didOpen").await?;
    let asked = manager.inlay_hints(
        Some("rust"),
        RequestId(41),
        DocumentId(1),
        1,
        &path,
        Range::default(),
    );
    assert!(asked, "the live server refused the request");
    await_method(&mut observed, "textDocument/inlayHint").await?;

    let _ = manager.document_closed(Some("rust"), &path);
    loop {
        let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
            .await?
            .ok_or("the retired task never answered")?;
        if let LspUpdate::InlayHints { request, hints, .. } = update {
            assert_eq!(request, RequestId(41));
            assert!(hints.is_empty());
            return Ok(());
        }
    }
}
