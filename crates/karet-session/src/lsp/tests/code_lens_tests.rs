//! The seam contract for [`Command::CodeLenses`] and [`Command::RunCodeLens`]:
//! a server's lenses arrive in buffer columns with the ones on the asked
//! lines resolved, every case without a server still answers, and a lens's
//! command runs on the server with its own arguments.

use karet_core::CodeLens;
use karet_core::CommandId;
use serde_json::Value;

use super::*;
use crate::api::AppliedWorkspaceEdit;
use crate::lsp::CodeLensAnswer;

/// Line 0 opens with an emoji: one buffer column, two UTF-16 code units, so a
/// column that survives the round trip unconverted lands one off.
const TEXT: &str = "😀abcd\nfn main() {}\n\n\n\nfn other() {}\n";

/// A session over `name` (holding [`TEXT`]) served by a double that behaves
/// as `behavior`, reporting what it receives to `observed`, with the document
/// open.
async fn opened(
    dir: &tempfile::TempDir,
    name: &str,
    behavior: Behavior,
    observed: Option<mpsc::UnboundedSender<Value>>,
) -> Result<(impl Backend, EventRx, DocumentId, u64), Box<dyn std::error::Error + Send + Sync>> {
    let path = rust_file(dir, name, TEXT).ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(behavior, observed, spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
    Ok((backend, events, doc, version))
}

/// What answered listing `request`: the lenses, or `None` for
/// [`Event::CodeLensesFailed`]; with the version it echoed, and whether a
/// [`Event::FeatureUnsupported`] came for it first.
async fn listing(
    events: &mut EventRx,
    request: RequestId,
) -> Result<(u64, Option<Vec<CodeLens>>, bool), Box<dyn std::error::Error + Send + Sync>> {
    let mut noticed = false;
    loop {
        let (id, event) = next_event(events).await.ok_or("no answer")?;
        match event {
            Event::FeatureUnsupported { .. } if id == Some(request) => noticed = true,
            Event::CodeLenses {
                version, lenses, ..
            } if id == Some(request) => return Ok((version, Some(lenses), noticed)),
            Event::CodeLensesFailed { version, .. } if id == Some(request) => {
                return Ok((version, None, noticed));
            },
            _ => {},
        }
    }
}

/// The `WorkspaceEdit` answering `request`, skipping everything else.
async fn ran(events: &mut EventRx, request: RequestId) -> Option<AppliedWorkspaceEdit> {
    while let Some((id, event)) = next_event(events).await {
        if let Event::WorkspaceEdit { applied } = event
            && id == Some(request)
        {
            return Some(applied);
        }
    }
    None
}

fn ask(
    backend: &impl Backend,
    doc: DocumentId,
) -> Result<RequestId, Box<dyn std::error::Error + Send + Sync>> {
    let request = backend.next_id();
    backend.send(request, Command::CodeLenses { doc, lines: (0, 2) })?;
    Ok(request)
}

#[tokio::test]
async fn lenses_arrive_in_buffer_columns_with_the_asked_lines_resolved() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) =
        opened(&dir, "main.rs", Behavior::Normal, None).await?;

    let request = ask(&backend, doc)?;
    let (at, lenses, noticed) = listing(&mut events, request).await?;

    assert_eq!(at, version);
    assert!(!noticed);
    let lenses = lenses.ok_or("the listing failed")?;
    let [run, near, far] = lenses.as_slice() else {
        return Err(format!("expected three lenses, got {lenses:?}").into());
    };
    // The server's UTF-16 2..4 is buffer columns 1..3, after the emoji.
    assert_eq!(
        run.range,
        Range::new(LineCol::new(0, 1), LineCol::new(0, 3)).map_err(|e| format!("{e}"))?
    );
    assert_eq!(run.title.as_deref(), Some("Run"));
    assert_eq!(run.command, Some(CommandId("double.run".into())));
    assert_eq!(
        near.title.as_deref(),
        Some("references 2"),
        "a deferred lens on an asked line is resolved"
    );
    assert_eq!(near.command, None, "a lens that only informs runs nothing");
    assert!(
        !far.is_resolved(),
        "a lens outside the asked lines costs no resolve"
    );
    Ok(())
}

#[tokio::test]
async fn a_server_without_lenses_answers_none_and_says_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "main.rs", Behavior::Bare, None).await?;

    let request = ask(&backend, doc)?;
    let (at, lenses, noticed) = listing(&mut events, request).await?;

    assert_eq!((at, lenses), (version, Some(Vec::new())));
    assert!(
        !noticed,
        "lenses are asked unprompted; a refusal is no news"
    );
    Ok(())
}

#[tokio::test]
async fn a_document_no_server_serves_answers_no_lenses() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) =
        opened(&dir, "notes.txt", Behavior::Normal, None).await?;

    let request = ask(&backend, doc)?;
    let (at, lenses, _) = listing(&mut events, request).await?;

    assert_eq!((at, lenses), (version, Some(Vec::new())));
    Ok(())
}

/// A clicked lens runs its command on the server with the arguments the
/// server gave it, and the answer is labelled with the lens's title. A lens
/// whose command the server does not run is refused under its title too.
#[tokio::test]
async fn a_lens_runs_its_command_and_a_foreign_one_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let (backend, mut events, doc, _) =
        opened(&dir, "main.rs", Behavior::Normal, Some(observed_tx)).await?;
    let request = ask(&backend, doc)?;
    let (_, lenses, _) = listing(&mut events, request).await?;
    let lens = lenses.and_then(|lenses| lenses.into_iter().next());
    let lens = lens.ok_or("the Run lens")?;

    let run = backend.next_id();
    backend.send(
        run,
        Command::RunCodeLens {
            doc,
            lens: lens.clone(),
        },
    )?;
    let applied = ran(&mut events, run).await.ok_or("no answer")?;
    assert!(applied.result.applied, "{applied:?}");
    assert_eq!(applied.label.as_deref(), Some("Run"));
    let mut command = None;
    while let Ok(Some(msg)) =
        tokio::time::timeout(Duration::from_millis(500), observed_rx.recv()).await
    {
        if msg["method"] == "workspace/executeCommand" {
            command = Some(msg["params"].clone());
        }
    }
    let command = command.ok_or("the command ran")?;
    assert_eq!(command["command"], "double.run");
    assert_eq!(command["arguments"], json!([1]));

    // A client-side command, as some servers put on lenses, is not sent.
    let mut foreign = lens;
    let mut literal: Value = serde_json::from_str(foreign.payload.as_deref().unwrap_or("{}"))?;
    literal["command"]["command"] = json!("editor.showReferences");
    foreign.payload = Some(literal.to_string());
    foreign.command = Some(CommandId("editor.showReferences".into()));
    let run = backend.next_id();
    backend.send(run, Command::RunCodeLens { doc, lens: foreign })?;
    let refused = ran(&mut events, run).await.ok_or("no answer")?;
    assert!(!refused.result.applied);
    assert_eq!(refused.label.as_deref(), Some("Run"));
    assert!(refused.result.failure_reason.is_some());
    Ok(())
}

/// A lens with nothing to run is answered as done, without asking anyone.
#[tokio::test]
async fn a_lens_that_only_informs_runs_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _) = opened(&dir, "notes.txt", Behavior::Normal, None).await?;
    let run = backend.next_id();
    let lens = CodeLens {
        title: Some("3 references".into()),
        ..CodeLens::default()
    };
    backend.send(run, Command::RunCodeLens { doc, lens })?;
    let applied = ran(&mut events, run).await.ok_or("no answer")?;
    assert!(applied.result.applied);
    assert_eq!(applied.label.as_deref(), Some("3 references"));
    Ok(())
}

/// A listing begun under a settings snapshot `reconfigure` has since retired
/// is fenced out, but still answered -- as unanswered, so the client asks
/// again rather than waiting, and keeps what it shows.
#[test]
fn a_listing_from_a_retired_generation_is_answered_unanswered() -> TestResult {
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

    session.apply_lsp_update(LspUpdate::CodeLens {
        generation: u64::MAX,
        answer: CodeLensAnswer::Listed {
            request: RequestId(2),
            doc,
            version,
            lenses: Some(vec![CodeLens::default()]),
        },
    });

    let mut answer = None;
    while let Some((id, event)) = events.try_recv() {
        match (id, event) {
            (Some(RequestId(2)), Event::CodeLensesFailed { version: at, .. }) => {
                answer = Some(at);
            },
            (Some(RequestId(2)), Event::CodeLenses { .. }) => {
                return Err("a fenced listing was adopted".into());
            },
            _ => {},
        }
    }
    assert_eq!(answer, Some(version));
    Ok(())
}
