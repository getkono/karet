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
/// Returns the document and its version.
async fn open_served(
    backend: &impl Backend,
    events: &mut EventRx,
    observed: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    path: PathBuf,
) -> Result<(DocumentId, u64), Box<dyn std::error::Error + Send + Sync>> {
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let opened = await_opened(events).await.ok_or("no Opened")?;
    await_method(observed, "textDocument/didOpen").await?;
    Ok(opened)
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

/// Wait for the answer to `request`, skipping answers to anything else: the
/// hints, or `None` for a request reported unanswered.
async fn await_hint_answer(
    events: &mut EventRx,
    request: RequestId,
) -> Option<Option<Vec<karet_core::InlayHint>>> {
    loop {
        let (id, event) = next_event(events).await?;
        if id != Some(request) {
            continue;
        }
        match event {
            Event::InlayHints { hints, .. } => return Some(Some(hints)),
            Event::InlayHintsFailed { .. } => return Some(None),
            _ => {},
        }
    }
}

/// A restart retires the slot while a hint request is running on it; the
/// request is reported unanswered rather than dropped -- and not answered
/// empty, which would blank the hints the editor shows until the new server
/// answers.
///
/// Falsified by: `HintFlight::shutdown` clearing its bookkeeping without
/// answering, as it once did -- the answer never arrives.
#[tokio::test]
async fn a_restart_reports_a_running_hint_request_unanswered() -> TestResult {
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
    let (doc, _) = open_served(&backend, &mut events, &mut observed, path).await?;

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
    assert_eq!(
        hints, None,
        "a retired request was answered as having no hints"
    );
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
        match update {
            LspUpdate::InlayHintsFailed { request, .. } => {
                assert_eq!(request, RequestId(41));
                return Ok(());
            },
            LspUpdate::InlayHints { .. } => {
                return Err("a retired request was answered as having no hints".into());
            },
            _ => {},
        }
    }
}

/// The connection is lost with a hint request in flight; the request is
/// reported unanswered, whichever notices first -- the liveness arm abandoning it,
/// or the request itself failing on the closed connection.
#[tokio::test]
async fn a_lost_connection_reports_a_running_hint_request_unanswered() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "let a = 1;\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(
        Behavior::DiesOnHint,
        Some(observed_tx),
        spawns,
    ));
    let backend = local_session(session, None);
    let (doc, _) = open_served(&backend, &mut events, &mut observed, path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::InlayHints {
            doc,
            range: whole_first_line(),
        },
    )?;
    let hints = await_hint_answer(&mut events, request)
        .await
        .ok_or("the lost connection left the hint request unanswered")?;
    assert_eq!(
        hints, None,
        "a lost request was answered as having no hints"
    );
    Ok(())
}

/// The pending `didChange` goes out once the last *edit* has been quiet for
/// the debounce, however many non-flushing commands keep arriving.
///
/// The editor asks for hints once an edit has been quiet for the same 150 ms,
/// and a hint request for the edited document waits for the flush rather than
/// forcing it. When the window was timed from the last command of any kind,
/// each such request restarted it -- so a steady stream of them, here one
/// every 50 ms, held the edit back for as long as the stream lasted.
///
/// Falsified by: re-arming `flush_at` on every command in `server_task`, as
/// the per-command timeout did -- the edit reaches the server only after the
/// stream stops, well past the bound.
#[tokio::test]
async fn non_flushing_commands_do_not_postpone_the_debounced_flush() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "lib.rs", "fn a() {}\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, version) = open_served(&backend, &mut events, &mut observed, path).await?;

    let at = Range::new(LineCol::new(1, 0), LineCol::new(1, 0)).map_err(|e| format!("{e}"))?;
    let edited = std::time::Instant::now();
    backend.send(
        backend.next_id(),
        Command::ApplyChange {
            doc,
            change: Change::new(
                version,
                vec![TextEdit {
                    range: at,
                    new_text: "x".to_owned(),
                }],
            ),
            cause: EditCause::Type,
        },
    )?;

    // Far longer than the debounce, and far shorter than the stream: a flush
    // that waited for the stream to stop lands after it, not inside it.
    let bound = Duration::from_secs(1);
    let stream = Duration::from_secs(3);
    let mut flushed = None;
    while flushed.is_none() && edited.elapsed() < stream {
        backend.send(
            backend.next_id(),
            Command::InlayHints {
                doc,
                range: whole_first_line(),
            },
        )?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(message) = observed.try_recv() {
            if message["method"] == "textDocument/didChange" {
                flushed = Some(edited.elapsed());
            }
        }
    }
    let flushed = flushed.ok_or("the edit was never flushed while commands kept arriving")?;
    assert!(
        flushed >= CHANGE_DEBOUNCE,
        "the edit was flushed before the debounce elapsed: {flushed:?}"
    );
    assert!(
        flushed < bound,
        "non-flushing commands postponed the flush to {flushed:?}"
    );
    Ok(())
}

/// While a server is being retried its hints are held -- the request is
/// reported unanswered -- but once its restart circuit opens they are dropped.
///
/// A server that keeps dying can sit behind an open circuit indefinitely, so
/// holding its last hints there would paint them stale forever.
#[tokio::test]
async fn hints_are_held_while_retrying_and_dropped_once_the_circuit_opens() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "let a = 1;\n").ok_or("write failed")?;
    // Transient on every attempt, so the task retries until the circuit opens
    // (five failures inside the window: about four seconds of back-off).
    let connector: Connector = Arc::new(|spec, _root| {
        let failure = karet_lsp::LaunchFailure::host(
            spec.command.clone(),
            spec.args.clone(),
            "shared broker unreachable",
        );
        Box::pin(async move { Err(LspError::Launch(Box::new(failure))) })
    });
    let (session, mut events) = session_with_connector(connector);
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;

    let mut held = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() > deadline {
            return Err("the circuit never opened, or its hints were never dropped".into());
        }
        let request = backend.next_id();
        backend.send(
            request,
            Command::InlayHints {
                doc,
                range: whole_first_line(),
            },
        )?;
        match await_hint_answer(&mut events, request).await {
            Some(None) => held += 1,
            Some(Some(hints)) => {
                assert!(hints.is_empty());
                break;
            },
            None => return Err("event stream ended".into()),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        held > 0,
        "hints were dropped while the server was still being retried"
    );
    Ok(())
}
