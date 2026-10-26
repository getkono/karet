//! The `ResolveInlayHint` → `InlayHintResolved` contract: a hint goes out
//! with its payload, comes back filled in, keeps the position it was asked
//! about, and has its parts' locations in buffer columns -- and a hint no
//! server can resolve still comes back, unchanged.

use super::*;

/// Open `path` and wait until the session confirms it.
async fn open(
    backend: &impl Backend,
    events: &mut EventRx,
    path: PathBuf,
) -> Result<(DocumentId, u64), Box<dyn std::error::Error + Send + Sync>> {
    backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path,
            language: None,
        }
        .into(),
    )?;
    Ok(await_opened(events).await.ok_or("no Opened")?)
}

/// The answer to `request`, skipping everything else, within a bound.
async fn answer_to(events: &mut EventRx, request: RequestId) -> Option<Event> {
    let answer = async {
        loop {
            let (id, event) = next_event(events).await?;
            if id == Some(request)
                && matches!(
                    event,
                    Event::InlayHints { .. } | Event::InlayHintResolved { .. }
                )
            {
                return Some(event);
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), answer)
        .await
        .ok()
        .flatten()
}

fn whole_first_line() -> Range {
    Range {
        start: LineCol::new(0, 0),
        end: LineCol::new(0, u32::MAX),
    }
}

/// Falsified by: the session dropping the payload on the way to the client
/// (the resolve never reaches the wire), forwarding the server's UTF-16
/// location unconverted (column 4, not 3), or answering with the server's
/// position instead of the one asked about.
#[tokio::test]
async fn a_hint_resolves_through_the_seam_in_buffer_columns() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "\u{1F600}ab = ab;\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path.clone()).await?;

    let request = backend.next_id();
    backend.send(
        request,
        LspCommand::InlayHints {
            doc,
            range: whole_first_line(),
        }
        .into(),
    )?;
    let Some(Event::InlayHints { hints, .. }) = answer_to(&mut events, request).await else {
        return Err("no hints".into());
    };
    let hint = hints
        .first()
        .cloned()
        .ok_or("the double answers one hint")?;
    assert!(hint.payload.is_some(), "a hint with data keeps its payload");
    assert_eq!(hint.tooltip, None);

    // Where the client holds it, which a resolve must not move.
    let held = karet_core::InlayHint {
        position: LineCol::new(0, 7),
        ..hint
    };
    let request = backend.next_id();
    backend.send(
        request,
        LspCommand::ResolveInlayHint {
            doc,
            hint: held.clone(),
        }
        .into(),
    )?;
    let Some(Event::InlayHintResolved {
        doc: answered,
        hint: resolved,
    }) = answer_to(&mut events, request).await
    else {
        return Err("no resolve answer".into());
    };
    assert_eq!(answered, doc);
    assert_eq!(resolved.position, held.position);
    assert_eq!(
        resolved.tooltip.as_ref().map(|t| t.value.as_str()),
        Some("resolved by the double")
    );
    let location = resolved
        .location_for(1)
        .ok_or("the type part leads somewhere")?;
    assert_eq!(location.path, path);
    assert_eq!(location.range.start, LineCol::new(0, 3));

    let wire = loop {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await?
            .ok_or("server stream ended")?;
        if message["method"] == "inlayHint/resolve" {
            break message;
        }
    };
    assert_eq!(wire["params"]["data"], serde_json::json!({"hint": 1}));
    Ok(())
}

/// A hint list's part locations are converted against the open file they
/// name, and left in the server's columns for a file that is not open, which
/// the list path never reads from disk.
///
/// Falsified by: the list path forwarding an open file's UTF-16 location
/// unconverted (column 4, not 3), or reading `elsewhere.rs` from disk to
/// convert its location (column 3, not 4).
#[tokio::test]
async fn hint_list_part_locations_convert_against_open_files_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "\u{1F600}ab = ab;\n").ok_or("write failed")?;
    let elsewhere = rust_file(&dir, "elsewhere.rs", "\u{1F600}cd = cd;\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path.clone()).await?;

    let request = backend.next_id();
    backend.send(
        request,
        LspCommand::InlayHints {
            doc,
            range: whole_first_line(),
        }
        .into(),
    )?;
    let Some(Event::InlayHints { hints, .. }) = answer_to(&mut events, request).await else {
        return Err("no hints".into());
    };
    let hint = hints.first().ok_or("the double answers one hint")?;
    let unopened = hint
        .location_for(0)
        .ok_or("the ': ' part leads somewhere")?;
    assert_eq!(unopened.path, elsewhere);
    assert_eq!(unopened.range.start, LineCol::new(0, 4));
    let open_target = hint
        .location_for(1)
        .ok_or("the type part leads somewhere")?;
    assert_eq!(open_target.path, path);
    assert_eq!(open_target.range.start, LineCol::new(0, 3));
    assert_eq!(open_target.range.end, LineCol::new(0, 4));
    Ok(())
}

/// A server that does not resolve hints is never asked, and the hint still
/// comes back, unchanged, so the client stops waiting on it.
///
/// Falsified by: the session leaving the command unanswered when the server
/// has nothing to add.
#[tokio::test]
async fn a_hint_no_server_resolves_comes_back_unchanged() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "let a = 1;\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Bare, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let hint = karet_core::InlayHint {
        payload: Some(r#"{"position":{"line":0,"character":5},"label":"x","data":1}"#.to_owned()),
        ..karet_core::InlayHint::new(LineCol::new(0, 5), "x", karet_core::InlayHintKind::Type)
    };
    let request = backend.next_id();
    backend.send(
        request,
        LspCommand::ResolveInlayHint {
            doc,
            hint: hint.clone(),
        }
        .into(),
    )?;
    let Some(Event::InlayHintResolved { hint: answered, .. }) =
        answer_to(&mut events, request).await
    else {
        return Err("no resolve answer".into());
    };
    assert_eq!(answered, hint);
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(300), observed.recv()).await
    {
        assert_ne!(message["method"], "inlayHint/resolve");
    }
    Ok(())
}
