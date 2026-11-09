//! The `Completion` → `Completions` and `ResolveCompletion` →
//! `CompletionResolved` contract: a list comes back with the server's
//! trigger and commit characters and every range in buffer columns, an item
//! goes out with its payload and comes back filled in, and an item no server
//! can resolve is still answered.

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
                    Event::Completions { .. } | Event::CompletionResolved { .. }
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

/// Falsified by: the session dropping the server's trigger or commit
/// characters, the item's payload, or the trigger on its way to the wire;
/// forwarding the resolve answer's UTF-16 import column unconverted (2, not
/// 1); or losing the documentation it filled in.
#[tokio::test]
async fn an_item_lists_and_resolves_through_the_seam_in_buffer_columns() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "\u{1F600}ab = ab;\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        LspCommand::Completion {
            doc,
            position: LineCol::new(0, 3),
            trigger: CompletionTrigger::Character('.'),
        }
        .into(),
    )?;
    let Some(Event::Completions {
        items,
        incomplete,
        options,
        ..
    }) = answer_to(&mut events, request).await
    else {
        return Err("no completions".into());
    };
    assert!(!incomplete, "the double answers a bare array");
    assert_eq!(options.trigger_characters, vec!['.']);
    assert_eq!(options.commit_characters, vec![';']);
    let item = items
        .first()
        .cloned()
        .ok_or("the double answers one item")?;
    assert!(
        item.payload.is_some(),
        "a resolvable item keeps its payload"
    );

    let request = backend.next_id();
    backend.send(
        request,
        LspCommand::ResolveCompletion {
            doc,
            item: item.clone(),
        }
        .into(),
    )?;
    let Some(Event::CompletionResolved {
        doc: answered,
        resolved,
    }) = answer_to(&mut events, request).await
    else {
        return Err("no resolve answer".into());
    };
    assert_eq!(answered, doc);
    let resolved = resolved.ok_or("the double resolves the item")?;
    assert_eq!(
        resolved.documentation.map(|docs| docs.value).as_deref(),
        Some("resolved by the double")
    );
    let import = resolved
        .additional_edits
        .first()
        .ok_or("the double adds an import")?;
    assert_eq!(import.range.start, LineCol::new(0, 1));
    assert_eq!(import.new_text, "use x;");

    let mut context = None;
    let wire = loop {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await?
            .ok_or("server stream ended")?;
        if message["method"] == "textDocument/completion" {
            context = Some(message["params"]["context"].clone());
        }
        if message["method"] == "completionItem/resolve" {
            break message;
        }
    };
    assert_eq!(
        context,
        Some(json!({"triggerKind": 2, "triggerCharacter": "."}))
    );
    assert_eq!(wire["params"]["data"], json!({"item": 1}));
    Ok(())
}

/// A server that does not resolve items is never asked, and the command is
/// still answered, so the client stops waiting.
///
/// Falsified by: the session leaving the command unanswered, or sending the
/// resolve to a server that never offered it.
#[tokio::test]
async fn an_item_no_server_resolves_is_answered_with_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "let a = 1;\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Bare, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let item = CompletionItem {
        payload: Some(r#"{"label":"x","data":1}"#.to_owned()),
        ..CompletionItem::new("x", karet_core::CompletionKind::Text)
    };
    let request = backend.next_id();
    backend.send(request, LspCommand::ResolveCompletion { doc, item }.into())?;
    let Some(Event::CompletionResolved { resolved, .. }) = answer_to(&mut events, request).await
    else {
        return Err("no resolve answer".into());
    };
    assert_eq!(resolved, None);
    while let Ok(message) = observed.try_recv() {
        assert_ne!(message["method"], "completionItem/resolve");
    }
    Ok(())
}
