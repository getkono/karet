//! The seam contract for [`LspCommand::DocumentHighlights`]: a server that offers
//! document highlights answers with its occurrences in buffer columns, and
//! every other case -- a server without them, no server at all, a document
//! that moved on, an answer fenced out by reconfiguration -- still answers,
//! with `None`, and raises no notice.

use super::*;
use crate::api::DocumentHighlight;
use crate::api::HighlightKind;
use crate::lsp::DocumentHighlightsAnswer;

/// Line 0 opens with an emoji: one buffer column, two UTF-16 code units, so
/// a column that survives the round trip unconverted lands one off.
const TEXT: &str = "😀abcd\nfn main() {}\n";

/// The caret asked about: `b` on line 0.
const CARET: LineCol = LineCol::new(0, 2);

fn range(start: (u32, u32), end: (u32, u32)) -> Range {
    Range {
        start: LineCol::new(start.0, start.1),
        end: LineCol::new(end.0, end.1),
    }
}

type Answer = (DocumentId, u64, Option<Vec<DocumentHighlight>>, bool);

/// A session over `name` (holding [`TEXT`]) served by a double that behaves
/// as `behavior`, with the document open.
async fn opened(
    dir: &tempfile::TempDir,
    name: &str,
    behavior: Behavior,
) -> Result<(impl Backend, EventRx, DocumentId, u64), Box<dyn std::error::Error + Send + Sync>> {
    let path = rust_file(dir, name, TEXT).ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(behavior, None, spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path,
            language: None,
        }
        .into(),
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
    Ok((backend, events, doc, version))
}

/// What answered `request`: the [`Event::DocumentHighlights`] fields, and
/// whether a [`Event::FeatureUnsupported`] came for it first.
async fn answer_to(
    events: &mut EventRx,
    request: RequestId,
) -> Result<Answer, Box<dyn std::error::Error + Send + Sync>> {
    let mut noticed = false;
    loop {
        let (id, event) = next_event(events).await.ok_or("no DocumentHighlights")?;
        match event {
            Event::FeatureUnsupported { .. } if id == Some(request) => noticed = true,
            Event::DocumentHighlights {
                doc,
                version,
                highlights,
            } if id == Some(request) => return Ok((doc, version, highlights, noticed)),
            _ => {},
        }
    }
}

fn ask(
    backend: &impl Backend,
    doc: DocumentId,
) -> Result<RequestId, Box<dyn std::error::Error + Send + Sync>> {
    let request = backend.next_id();
    let position = CARET;
    backend.send(
        request,
        LspCommand::DocumentHighlights { doc, position }.into(),
    )?;
    Ok(request)
}

#[tokio::test]
async fn a_supporting_server_answers_its_occurrences_in_buffer_columns() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "main.rs", Behavior::Normal).await?;

    let request = ask(&backend, doc)?;
    let (answered, at, highlights, noticed) = answer_to(&mut events, request).await?;

    assert_eq!((answered, at), (doc, version));
    assert!(!noticed, "a server that offers it was reported as not");
    // The double echoes the caret's UTF-16 column, so `b` comes back as
    // buffer column 2 only if it went out as UTF-16 column 3 and was
    // converted back.
    assert_eq!(
        highlights,
        Some(vec![
            DocumentHighlight {
                range: range((0, 2), (0, 3)),
                kind: HighlightKind::Write,
            },
            DocumentHighlight {
                range: range((1, 0), (1, 2)),
                kind: HighlightKind::Text,
            },
        ])
    );
    Ok(())
}

#[tokio::test]
async fn a_server_without_document_highlights_answers_none_silently() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "main.rs", Behavior::Bare).await?;

    let request = ask(&backend, doc)?;
    let (answered, at, highlights, noticed) = answer_to(&mut events, request).await?;

    assert_eq!((answered, at), (doc, version));
    assert_eq!(highlights, None);
    assert!(
        !noticed,
        "the client falls back on its own; nothing to report"
    );
    Ok(())
}

#[tokio::test]
async fn a_document_no_server_serves_answers_none() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "notes.txt", Behavior::Normal).await?;

    let request = ask(&backend, doc)?;
    let (answered, at, highlights, noticed) = answer_to(&mut events, request).await?;

    assert_eq!((answered, at), (doc, version));
    assert_eq!(highlights, None);
    assert!(!noticed);
    Ok(())
}

#[tokio::test]
async fn an_answer_to_a_document_that_moved_on_carries_no_highlights() -> TestResult {
    // The edit is queued behind the request, so the actor applies it before
    // the server's answer can come back: the answer then describes text that
    // is gone, and is delivered as `None` under the version it was asked at.
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "main.rs", Behavior::Normal).await?;

    let request = ask(&backend, doc)?;
    let at = Range::new(LineCol::new(1, 0), LineCol::new(1, 0));
    backend.send(
        backend.next_id(),
        DocumentCommand::ApplyChange {
            doc,
            change: Change::new(
                version,
                vec![TextEdit {
                    range: at,
                    new_text: "x".to_owned(),
                }],
            ),
            cause: EditCause::Type,
        }
        .into(),
    )?;
    let (answered, asked_at, highlights, _) = answer_to(&mut events, request).await?;

    assert_eq!((answered, asked_at), (doc, version));
    assert_eq!(highlights, None);
    Ok(())
}

/// An answer begun under a settings snapshot that `reconfigure` has since
/// retired is fenced out, but the request it answers is still answered --
/// with `None` -- so the client falls back rather than waiting.
#[test]
fn an_answer_from_a_retired_generation_still_answers_none() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "notes.txt", TEXT).ok_or("write failed")?;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    session.handle(
        RequestId(1),
        DocumentCommand::Open {
            path,
            language: None,
        },
    );
    let doc = DocumentId(1);
    let version = session
        .document(doc)
        .map(|d| d.version())
        .ok_or("no document")?;

    session.apply_lsp_update(LspUpdate::DocumentHighlights {
        generation: u64::MAX,
        answer: DocumentHighlightsAnswer {
            request: RequestId(2),
            doc,
            version,
            highlights: Some(vec![DocumentHighlight {
                range: range((0, 0), (0, 1)),
                kind: HighlightKind::Read,
            }]),
        },
    });

    let mut answer = None;
    while let Some((id, event)) = events.try_recv() {
        if let (Some(RequestId(2)), Event::DocumentHighlights { highlights, .. }) = (id, event) {
            answer = Some(highlights);
        }
    }
    assert_eq!(answer, Some(None));
    Ok(())
}
