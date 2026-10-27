//! The seam contract for language intelligence beyond a plain round trip: a
//! server asking for its hints to be refreshed, and a server that does not
//! offer what it is asked for.
//!
//! Split from `roundtrip_tests`, which proves the columns survive the wire.
//! These prove which *events* a client sees, and in what order.

use super::*;
use crate::api::LocationKind;

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
    let at = Range::new(LineCol::new(1, 0), LineCol::new(1, 0));
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
            trigger: CompletionTrigger::Invoked,
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

// On tokio's virtual clock, so the closing silence window proves absence: the
// server is in-process, and idle time advances only once every task has run, so
// a request already launched reaches the wire before the window can close.
#[tokio::test(start_paused = true)]
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
    // A background request is refused *silently*: no notice precedes the
    // empty answer, or every viewport change would raise one.
    let (rid, hdoc, hversion, hints) = loop {
        let (id, event) = next_event(&mut events).await.ok_or("no InlayHints event")?;
        match event {
            Event::FeatureUnsupported { .. } => {
                return Err("a background request raised an unsupported notice".into());
            },
            Event::InlayHints {
                doc,
                version,
                hints,
            } => break (id, doc, version, hints),
            _ => {},
        }
    };
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

#[tokio::test]
async fn a_request_asked_for_by_hand_says_the_server_does_not_support_it() -> TestResult {
    // Issue #279: "this server does not support X" must read differently from
    // "this server found nothing". The notice comes first, tagged with the
    // request, and the empty answer the caller is owed still follows it.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Bare, None, spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::Definition {
            doc,
            position: LineCol::new(0, 3),
        },
    )?;
    let mut notice = None;
    loop {
        let (id, event) = next_event(&mut events)
            .await
            .ok_or("no Definitions answer")?;
        match event {
            Event::FeatureUnsupported { server, feature } => {
                assert_eq!(id, Some(request), "the notice names its request");
                notice = Some((server, feature));
            },
            Event::Definitions { locations } => {
                assert_eq!(id, Some(request));
                assert!(locations.is_empty());
                break;
            },
            _ => {},
        }
    }
    assert_eq!(
        notice,
        Some((
            LanguageServerId::RustAnalyzer,
            karet_core::ServerFeature::Definition
        )),
        "the refusal was not surfaced ahead of its empty answer"
    );
    Ok(())
}

#[tokio::test]
async fn find_references_answers_with_its_kind_in_buffer_columns() -> TestResult {
    // The whole seam for the new navigation command: it reaches the wire as
    // `textDocument/references` asking for the declaration too, and its answer
    // comes back as `Locations` naming what was asked -- in buffer columns,
    // since the server counted the leading emoji as two.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "\u{1F600}ab = ab;\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path.clone()).await?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::FindLocations {
            doc,
            position: LineCol::new(0, 1),
            kind: LocationKind::References,
        },
    )?;
    let (kind, locations) = loop {
        let (id, event) = next_event(&mut events).await.ok_or("no Locations answer")?;
        match event {
            Event::FeatureUnsupported { .. } => {
                return Err("a server that offers references was reported as not".into());
            },
            Event::Locations { kind, locations } => {
                assert_eq!(id, Some(request));
                break (kind, locations);
            },
            _ => {},
        }
    };
    assert_eq!(kind, LocationKind::References);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start, LineCol::new(0, 3));

    let wire = loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), observed_rx.recv())
            .await?
            .ok_or("server stream ended")?;
        if msg["method"] == "textDocument/references" {
            break msg;
        }
    };
    assert_eq!(wire["params"]["context"]["includeDeclaration"], true);
    // The caret's buffer column 1 is UTF-16 character 2 on the wire.
    assert_eq!(
        wire["params"]["position"],
        json!({"line": 0, "character": 2})
    );
    Ok(())
}

#[tokio::test]
async fn an_unoffered_navigation_says_which_and_answers_empty_with_its_kind() -> TestResult {
    // The double offers no declarations: the notice names that capability,
    // not Go to Definition's, and the empty answer still carries its kind so
    // the client can tell it apart from a definition.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::FindLocations {
            doc,
            position: LineCol::new(0, 3),
            kind: LocationKind::Declaration,
        },
    )?;
    let mut notice = None;
    loop {
        let (id, event) = next_event(&mut events).await.ok_or("no Locations answer")?;
        match event {
            Event::FeatureUnsupported { feature, .. } => {
                assert_eq!(id, Some(request));
                notice = Some(feature);
            },
            Event::Locations { kind, locations } => {
                assert_eq!(id, Some(request));
                assert_eq!(kind, LocationKind::Declaration);
                assert!(locations.is_empty());
                break;
            },
            Event::Definitions { .. } => return Err("answered as a definition".into()),
            _ => {},
        }
    }
    assert_eq!(notice, Some(karet_core::ServerFeature::Declaration));
    Ok(())
}

#[tokio::test]
async fn a_supported_request_raises_no_notice() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::Hover {
            doc,
            position: LineCol::new(0, 3),
        },
    )?;
    loop {
        let (_, event) = next_event(&mut events)
            .await
            .ok_or("no HoverResult answer")?;
        match event {
            Event::FeatureUnsupported { .. } => {
                return Err("a server that offers hover was reported as not".into());
            },
            Event::HoverResult { .. } => return Ok(()),
            _ => {},
        }
    }
}

#[tokio::test]
async fn a_to_end_of_line_hint_request_reaches_the_wire_as_the_real_end() -> TestResult {
    // The editor asks through the end of its last visible line as
    // `u32::MAX`. Converted verbatim that is a column no document has, and a
    // server may reject the whole request; clamped, it is the line's end in
    // UTF-16 units -- 4 here, since the leading emoji is two of them.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "\u{1F600}ab\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    backend.send(
        backend.next_id(),
        Command::InlayHints {
            doc,
            range: whole_first_line(),
        },
    )?;
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), observed_rx.recv())
            .await?
            .ok_or("server stream ended")?;
        if msg["method"] == "textDocument/inlayHint" {
            assert_eq!(
                msg["params"]["range"]["end"],
                json!({"line": 0, "character": 4})
            );
            return Ok(());
        }
    }
}

/// Ask for signature help in `doc` and collect the answer, with any
/// refusal notice that came ahead of it.
async fn ask_signature_help(
    backend: &impl Backend,
    events: &mut EventRx,
    (doc, position, manual): (DocumentId, LineCol, bool),
) -> Result<
    (
        Option<karet_core::SignatureHelp>,
        karet_core::SignatureHelpOptions,
        Option<karet_core::ServerFeature>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let request = backend.next_id();
    backend.send(
        request,
        Command::SignatureHelp {
            doc,
            position,
            manual,
        },
    )?;
    let mut notice = None;
    loop {
        let (id, event) = next_event(events).await.ok_or("no SignatureHelp answer")?;
        match event {
            Event::FeatureUnsupported { feature, .. } if id == Some(request) => {
                notice = Some(feature);
            },
            Event::SignatureHelp {
                doc: answered,
                help,
                options,
            } if id == Some(request) => {
                assert_eq!(answered, doc, "the answer names the document asked about");
                return Ok((help, options, notice));
            },
            _ => {},
        }
    }
}

#[tokio::test]
async fn signature_help_reaches_the_client_with_the_servers_characters() -> TestResult {
    // The parameter labels arrive as UTF-16 offsets into the signature label
    // and leave as the text they cover; the trigger characters the client
    // keys its auto-trigger on travel beside them.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() { add(1, 2) }\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let at = (doc, LineCol::new(0, 19), false);
    let (help, options, notice) = ask_signature_help(&backend, &mut events, at).await?;
    let help = help.ok_or("the server's signature was dropped")?;
    assert_eq!(notice, None, "a server that offers it was reported as not");
    assert_eq!(help.active_parameter, 1);
    let labels: Vec<&str> = help.signatures[0]
        .parameters
        .iter()
        .map(|parameter| parameter.label.as_str())
        .collect();
    assert_eq!(labels, ["a: i32", "b: i32"]);
    assert_eq!(options.trigger_characters, ['(']);
    assert_eq!(options.retrigger_characters, [',']);
    Ok(())
}

#[tokio::test]
async fn only_a_manual_signature_help_request_reports_a_server_without_it() -> TestResult {
    // A typed `(` asks on every call; a server that has no signature help
    // must not raise a notice each time. Asked for by hand, it says why the
    // answer is empty -- and both are still answered.
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() { f(1) }\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Bare, None, spawns));
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;

    let typed =
        ask_signature_help(&backend, &mut events, (doc, LineCol::new(0, 14), false)).await?;
    assert_eq!(
        typed,
        (None, karet_core::SignatureHelpOptions::default(), None)
    );
    let manual =
        ask_signature_help(&backend, &mut events, (doc, LineCol::new(0, 14), true)).await?;
    assert_eq!(manual.0, None);
    assert_eq!(manual.2, Some(karet_core::ServerFeature::SignatureHelp));
    Ok(())
}
