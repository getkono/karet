//! The seam contract for the requests an edit or a save makes:
//! [`Command::LinkedEditingRanges`] and [`Command::FormatOnType`] are always
//! answered, in buffer columns, and a save asks for the server's will-save
//! edits and writes them, formatting after them when it formats.

use super::*;

/// Line 0 opens with an emoji: one buffer column, two UTF-16 code units, so
/// a column that survives the round trip unconverted lands one off.
const TEXT: &str = "😀abcd\nfn main() {}\n";

/// The caret asked about: `b` on line 0.
const CARET: LineCol = LineCol::new(0, 2);

type Failure = Box<dyn std::error::Error + Send + Sync>;

fn range(start: (u32, u32), end: (u32, u32)) -> Range {
    Range {
        start: LineCol::new(start.0, start.1),
        end: LineCol::new(end.0, end.1),
    }
}

/// A session over `name` (holding `text`) served by a double that behaves as
/// `behavior`, with `editor.formatOnSave` as `format`, and the document open.
async fn opened(
    dir: &tempfile::TempDir,
    (name, text): (&str, &str),
    (behavior, observed): (
        Behavior,
        Option<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
    ),
    format: bool,
) -> Result<(impl Backend, EventRx, DocumentId, u64, PathBuf), Failure> {
    let path = rust_file(dir, name, text).ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let mut settings = crate::config::Settings::default();
    settings.editor.format_on_save = format;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig {
        settings,
        ..SessionConfig::default()
    });
    session.set_lsp_connector(test_connector(behavior, observed, spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: path.clone(),
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
    Ok((backend, events, doc, version, path))
}

/// The linked ranges answering `request`, and whether a
/// [`Event::FeatureUnsupported`] came for it first.
async fn linked_answer(
    events: &mut EventRx,
    request: RequestId,
) -> Result<(u64, Option<Vec<Range>>, bool), Failure> {
    let mut noticed = false;
    loop {
        let (id, event) = next_event(events).await.ok_or("no LinkedEditingRanges")?;
        match event {
            Event::FeatureUnsupported { .. } if id == Some(request) => noticed = true,
            Event::LinkedEditingRanges {
                version, ranges, ..
            } if id == Some(request) => return Ok((version, ranges, noticed)),
            _ => {},
        }
    }
}

/// The on-type edits and trigger characters answering `request`, and whether
/// a [`Event::FeatureUnsupported`] came for it first.
async fn on_type_answer(
    events: &mut EventRx,
    request: RequestId,
) -> Result<(Vec<TextEdit>, Option<Vec<char>>, bool), Failure> {
    let mut noticed = false;
    loop {
        let (id, event) = next_event(events).await.ok_or("no OnTypeFormatting")?;
        match event {
            Event::FeatureUnsupported { .. } if id == Some(request) => noticed = true,
            Event::OnTypeFormatting {
                edits, triggers, ..
            } if id == Some(request) => return Ok((edits, triggers, noticed)),
            _ => {},
        }
    }
}

fn ask_linked(backend: &impl Backend, doc: DocumentId) -> Result<RequestId, Failure> {
    let request = backend.next_id();
    let position = CARET;
    backend.send(request, Command::LinkedEditingRanges { doc, position })?;
    Ok(request)
}

fn ask_on_type(backend: &impl Backend, doc: DocumentId, ch: char) -> Result<RequestId, Failure> {
    let request = backend.next_id();
    let position = CARET;
    backend.send(request, Command::FormatOnType { doc, position, ch })?;
    Ok(request)
}

/// Save `doc` and wait for the write; the text then on disk.
async fn save(
    backend: &impl Backend,
    events: &mut EventRx,
    doc: DocumentId,
    path: &Path,
) -> Result<String, Failure> {
    let request = backend.next_id();
    let cause = crate::api::SaveCause::Manual;
    backend.send(request, Command::Save { doc, cause })?;
    loop {
        let (id, event) = next_event(events).await.ok_or("no Saved")?;
        if matches!(event, Event::Saved { .. }) && id == Some(request) {
            return Ok(std::fs::read_to_string(path)?);
        }
    }
}

#[tokio::test]
async fn linked_ranges_arrive_in_buffer_columns() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version, _) =
        opened(&dir, ("main.rs", TEXT), (Behavior::Normal, None), false).await?;

    let request = ask_linked(&backend, doc)?;
    let (at, ranges, noticed) = linked_answer(&mut events, request).await?;

    assert_eq!(at, version);
    assert!(!noticed);
    // The double echoes the caret's UTF-16 column, so `b` comes back as
    // buffer column 2 only if it went out as 3 and was converted back.
    assert_eq!(
        ranges,
        Some(vec![range((0, 2), (0, 3)), range((1, 0), (1, 1))])
    );
    Ok(())
}

#[tokio::test]
async fn linked_editing_a_server_does_not_offer_is_reported_and_answered() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _, _) =
        opened(&dir, ("main.rs", TEXT), (Behavior::Bare, None), false).await?;

    let request = ask_linked(&backend, doc)?;
    let (_, ranges, noticed) = linked_answer(&mut events, request).await?;

    assert_eq!(ranges, None);
    assert!(noticed, "Start Linked Editing is asked by hand: say why");
    Ok(())
}

#[tokio::test]
async fn linked_editing_with_no_server_answers_none() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _, _) =
        opened(&dir, ("notes.txt", TEXT), (Behavior::Normal, None), false).await?;

    let request = ask_linked(&backend, doc)?;
    let (_, ranges, noticed) = linked_answer(&mut events, request).await?;

    assert_eq!(ranges, None);
    assert!(!noticed);
    Ok(())
}

#[tokio::test]
async fn a_trigger_character_answers_the_servers_edits_and_triggers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _, _) =
        opened(&dir, ("main.rs", TEXT), (Behavior::Normal, None), false).await?;

    let request = ask_on_type(&backend, doc, ';')?;
    let (edits, triggers, noticed) = on_type_answer(&mut events, request).await?;

    assert_eq!(
        edits,
        vec![TextEdit {
            range: range((0, 2), (0, 2)),
            new_text: "!".to_owned(),
        }]
    );
    assert_eq!(triggers, Some(vec![';', '}']));
    assert!(!noticed);
    Ok(())
}

/// The session leaves the text alone: the client applies the edits, since
/// only it knows whether the user typed on meanwhile.
#[tokio::test]
async fn on_type_edits_are_not_applied_by_the_session() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version, _) =
        opened(&dir, ("main.rs", TEXT), (Behavior::Normal, None), false).await?;

    let request = ask_on_type(&backend, doc, ';')?;
    let _ = on_type_answer(&mut events, request).await?;
    let probe = ask_linked(&backend, doc)?;
    let (at, _, _) = linked_answer(&mut events, probe).await?;

    assert_eq!(at, version, "the document must still be at its version");
    Ok(())
}

#[tokio::test]
async fn a_character_the_server_does_not_format_on_answers_only_the_triggers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _, _) =
        opened(&dir, ("main.rs", TEXT), (Behavior::Normal, None), false).await?;

    let request = ask_on_type(&backend, doc, 'x')?;
    let (edits, triggers, _) = on_type_answer(&mut events, request).await?;

    assert!(edits.is_empty());
    assert_eq!(triggers, Some(vec![';', '}']));
    Ok(())
}

/// A server without on-type formatting answers with a known, empty trigger
/// list, so the client stops offering keystrokes. Falsifier: answered `None`
/// ("not asked"), the client would offer every keystroke for the session.
#[tokio::test]
async fn on_type_formatting_a_server_does_not_offer_is_answered_silently() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _, _) =
        opened(&dir, ("main.rs", TEXT), (Behavior::Bare, None), false).await?;

    let request = ask_on_type(&backend, doc, ';')?;
    let (edits, triggers, noticed) = on_type_answer(&mut events, request).await?;

    assert!(edits.is_empty());
    assert_eq!(triggers, Some(Vec::new()), "the server formats on none");
    assert!(!noticed, "typing must never raise a notice");
    Ok(())
}

/// With no server to ask, nothing is known about the triggers: `None`, so
/// the client asks again once a server may be serving the document.
#[tokio::test]
async fn on_type_formatting_with_no_server_knows_no_triggers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _, _) =
        opened(&dir, ("notes.txt", TEXT), (Behavior::Normal, None), false).await?;

    let request = ask_on_type(&backend, doc, ';')?;
    let (edits, triggers, _) = on_type_answer(&mut events, request).await?;

    assert!(edits.is_empty());
    assert_eq!(triggers, None);
    Ok(())
}

#[tokio::test]
async fn on_type_edits_for_text_that_moved_on_are_dropped() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version, _) =
        opened(&dir, ("main.rs", TEXT), (Behavior::Normal, None), false).await?;

    let request = ask_on_type(&backend, doc, ';')?;
    let at = range((1, 0), (1, 0));
    let change = Change::new(
        version,
        vec![TextEdit {
            range: at,
            new_text: "x".to_owned(),
        }],
    );
    let cause = EditCause::Type;
    backend.send(
        backend.next_id(),
        Command::ApplyChange { doc, change, cause },
    )?;
    let (edits, _, _) = on_type_answer(&mut events, request).await?;

    assert!(edits.is_empty());
    Ok(())
}

/// The will-save edit lands first, and formatting then runs against the
/// text it produced: the server is sent that text before it is asked to
/// format. Asked of the text before it, a real formatter's edits would be
/// computed for lines that have since moved.
#[tokio::test]
async fn a_save_writes_the_will_save_edits_then_formats() -> TestResult {
    let dir = tempfile::tempdir()?;
    let text = ("main.rs", "original\nsecond\n");
    let (tx, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let (backend, mut events, doc, _, path) =
        opened(&dir, text, (Behavior::WillSaves, Some(tx)), true).await?;

    let written = save(&backend, &mut events, doc, &path).await?;

    assert_eq!(written, "formatted\n// saved\nsecond\n");
    // The first change to carry the edit; the formatted text follows it.
    let mut order = Vec::new();
    while let Ok(message) = observed.try_recv() {
        match message["method"].as_str() {
            Some("textDocument/willSaveWaitUntil") => order.push("will-save"),
            Some("textDocument/didChange")
                if message.to_string().contains("// saved") && !order.contains(&"sent") =>
            {
                order.push("sent");
            },
            Some("textDocument/formatting") => order.push("format"),
            _ => {},
        }
    }
    assert_eq!(order, ["will-save", "sent", "format"]);
    Ok(())
}

#[tokio::test]
async fn a_save_that_does_not_format_still_writes_the_will_save_edits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let text = ("main.rs", "original\nsecond\n");
    let (backend, mut events, doc, _, path) =
        opened(&dir, text, (Behavior::WillSaves, None), false).await?;

    let written = save(&backend, &mut events, doc, &path).await?;

    assert_eq!(written, "original\n// saved\nsecond\n");
    Ok(())
}

/// Falsifier: were the save to format anyway, or fall back on the built-in
/// formatter, the text would change.
#[tokio::test]
async fn a_save_with_no_will_save_and_no_formatting_writes_the_text_as_is() -> TestResult {
    let dir = tempfile::tempdir()?;
    let text = ("main.rs", "original\nsecond\n");
    let (backend, mut events, doc, _, path) =
        opened(&dir, text, (Behavior::Formats, None), false).await?;

    let written = save(&backend, &mut events, doc, &path).await?;

    assert_eq!(written, "original\nsecond\n");
    Ok(())
}

/// A server that never answers `willSaveWaitUntil` costs the save one
/// deadline, not two: the task gives up on the will-save request at the
/// save's own bound, asks for no formatting after it (the save has been
/// written by then), answers the save unformatted, and is free for the next
/// command.
///
/// The clock is tokio's virtual one, so the assertion is on which timer
/// ended the wait. Falsifier: a task that gave each request its own deadline
/// would format after the will-save timeout and wait twice as long.
#[tokio::test(start_paused = true)]
async fn a_will_save_that_never_answers_spends_the_one_deadline() -> TestResult {
    let (tx, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    let spawns = Arc::new(AtomicUsize::new(0));
    manager.set_connector(test_connector(Behavior::WillSavesNever, Some(tx), spawns));
    let path = PathBuf::from("/tmp/stalls-on-will-save.rs");
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    let save_timeout = Duration::from_secs(2);
    let ask = crate::lsp::FormattingAsk {
        will_save: Some(karet_lsp::SaveReason::Manual),
        save_timeout,
        ..crate::lsp::FormattingAsk::default()
    };
    let request = (RequestId(1), DocumentId(1), 1);
    let (id, doc, version) = request;
    assert!(
        manager.formatting(Some("rust"), id, doc, version, &path, ask),
        "the request must reach a server for this to test anything"
    );

    let started = tokio::time::Instant::now();
    let answer = loop {
        let next = tokio::time::timeout(Duration::from_secs(60), updates.recv())
            .await
            .map_err(|_elapsed| "the server task never answered the save")?;
        match next {
            Some(LspUpdate::Formatting {
                request,
                formatted,
                edits,
                ..
            }) => break (request, formatted, edits),
            Some(_other) => continue,
            None => return Err("the server task ended without answering".into()),
        }
    };
    let waited = started.elapsed();

    assert_eq!(answer, (RequestId(1), false, Vec::new()));
    assert!(
        waited <= crate::lsp::runtime::formatting_deadline(save_timeout),
        "both requests must share the save's one deadline (waited {waited:?})"
    );

    // The task is back, and served the completion asked after the save.
    let position = (LineCol::new(0, 0), karet_core::CompletionTrigger::Invoked);
    let (id, doc, version) = (RequestId(2), DocumentId(1), 1);
    assert!(manager.completion(Some("rust"), id, doc, version, &path, position));
    loop {
        match updates.recv().await {
            Some(LspUpdate::Completions { list, .. }) => {
                assert_eq!(list.items.len(), 1, "the server is still answering");
                break;
            },
            Some(_other) => continue,
            None => return Err("the server task was left wedged on the will-save".into()),
        }
    }
    let mut methods = Vec::new();
    while let Ok(message) = observed.try_recv() {
        if let Some(method) = message["method"].as_str() {
            methods.push(method.to_owned());
        }
    }
    assert!(
        methods
            .iter()
            .any(|m| m == "textDocument/willSaveWaitUntil")
    );
    assert!(
        !methods.iter().any(|m| m == "textDocument/formatting"),
        "nothing is formatted once the deadline has passed: {methods:?}"
    );
    Ok(())
}

/// The reason sent with `willSaveWaitUntil` is the save's cause: a save on
/// losing focus is `FocusOut` (3), and a manual or unrecognised one is
/// `Manual` (1). Falsifier: a mapping that sent every save as manual, or
/// swapped two reasons, would pass every other test here.
#[tokio::test]
async fn a_save_tells_the_server_why_it_saves() -> TestResult {
    use crate::api::SaveCause;
    for (cause, reason) in [
        (SaveCause::FocusChange, 3),
        (SaveCause::Manual, 1),
        (SaveCause::Unknown, 1),
    ] {
        let dir = tempfile::tempdir()?;
        let text = ("main.rs", "original\nsecond\n");
        let (tx, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let (backend, mut events, doc, _, _) =
            opened(&dir, text, (Behavior::WillSaves, Some(tx)), false).await?;
        let request = backend.next_id();
        backend.send(request, Command::Save { doc, cause })?;
        loop {
            let (id, event) = next_event(&mut events).await.ok_or("no Saved")?;
            if matches!(event, Event::Saved { .. }) && id == Some(request) {
                break;
            }
        }
        let mut sent = Vec::new();
        while let Ok(message) = observed.try_recv() {
            if message["method"] == "textDocument/willSaveWaitUntil" {
                sent.push(message["params"]["reason"].as_u64());
            }
        }
        assert_eq!(sent, [Some(reason)], "a {cause:?} save");
    }
    Ok(())
}

/// Answers begun under a settings snapshot that `reconfigure` has since
/// retired are fenced out, but each request is still answered, with nothing.
#[test]
fn answers_from_a_retired_generation_still_answer_with_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "notes.txt", TEXT).ok_or("write failed")?;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    session.handle(
        RequestId(1),
        Command::OpenDocument {
            path,
            language: None,
        },
    );
    let doc = DocumentId(1);
    let version = session
        .document(doc)
        .map(|d| d.version())
        .ok_or("no document")?;
    let retired = |request, reply| LspUpdate::Editing {
        generation: u64::MAX,
        answer: crate::lsp::EditingAnswer {
            request: RequestId(request),
            doc,
            version,
            reply,
        },
    };
    let ranges = Some(vec![range((0, 0), (0, 1))]);
    session.apply_lsp_update(retired(2, crate::lsp::EditingReply::Linked(ranges)));
    let edits = vec![TextEdit {
        range: range((0, 0), (0, 0)),
        new_text: "!".to_owned(),
    }];
    let triggers = Some(vec![';']);
    let reply = crate::lsp::EditingReply::OnType { edits, triggers };
    session.apply_lsp_update(retired(3, reply));

    let (mut linked, mut on_type) = (None, None);
    while let Some((id, event)) = events.try_recv() {
        match (id, event) {
            (Some(RequestId(2)), Event::LinkedEditingRanges { ranges, .. }) => {
                linked = Some(ranges);
            },
            (
                Some(RequestId(3)),
                Event::OnTypeFormatting {
                    edits, triggers, ..
                },
            ) => on_type = Some((edits, triggers)),
            _ => {},
        }
    }
    assert_eq!(linked, Some(None));
    assert_eq!(on_type, Some((Vec::new(), None)));
    Ok(())
}
