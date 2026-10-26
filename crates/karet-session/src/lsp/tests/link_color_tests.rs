//! The seam contract for [`Command::LinksAndColors`] and
//! [`Command::ResolveDocumentLink`]: a server's links and colours arrive in
//! buffer columns, every case without a server still answers, and a deferred
//! link resolves through the server with its own data.

use karet_core::DocumentColor;
use karet_core::DocumentLink;
use serde_json::Value;

use super::*;
use crate::lsp::LinkColorAnswer;

/// Line 0 opens with an emoji: one buffer column, two UTF-16 code units, so a
/// column that survives the round trip unconverted lands one off.
const TEXT: &str = "😀abcd\nfn main() {}\n";

type Halves = (Option<Vec<DocumentLink>>, Option<Vec<DocumentColor>>);

/// The server double's answer to the document-link and colour methods.
///
/// Listing links offers two: one to `https://example.com/` over UTF-16
/// characters 2..4 of line 0 (buffer columns 1..3 past [`TEXT`]'s emoji),
/// and a deferred one on line 1. Resolving targets a link `file:///w/<n>.rs`
/// from its `data`, except `n` = 0, which it leaves without a target.
/// Listing colours offers red over line 0's UTF-16 characters 2..4.
pub(super) fn double_reply(msg: &Value) -> Value {
    let on = |line: u32, from: u32, to: u32| json!({"start": {"line": line, "character": from}, "end": {"line": line, "character": to}});
    match msg["method"].as_str() {
        Some("textDocument/documentLink") => json!([
            {"range": on(0, 2, 4), "target": "https://example.com/", "tooltip": "Example"},
            {"range": on(1, 0, 2), "data": {"n": 2}}
        ]),
        Some("documentLink/resolve") => {
            let mut resolved = msg["params"].clone();
            let n = &msg["params"]["data"]["n"];
            if n != &json!(0) {
                resolved["target"] = json!(format!("file:///w/{n}.rs"));
            }
            resolved
        },
        Some("textDocument/documentColor") => json!([
            {"range": on(0, 2, 4), "color": {"red": 1.0, "green": 0.0, "blue": 0.0, "alpha": 1.0}}
        ]),
        // Answered from the same arm of the double, which is at its line
        // ceiling; the ranges are the folding tests'.
        Some("textDocument/foldingRange") => super::folding_tests::double_reply(),
        _ => Value::Null,
    }
}

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
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
    Ok((backend, events, doc, version))
}

/// What answered listing `request`, with the version it echoed, and whether
/// a [`Event::FeatureUnsupported`] came for it first.
async fn listing(
    events: &mut EventRx,
    request: RequestId,
) -> Result<(u64, Halves, bool), Box<dyn std::error::Error + Send + Sync>> {
    let mut noticed = false;
    loop {
        let (id, event) = next_event(events).await.ok_or("no answer")?;
        match event {
            Event::FeatureUnsupported { .. } if id == Some(request) => noticed = true,
            Event::LinksAndColors {
                version,
                links,
                colors,
                ..
            } if id == Some(request) => return Ok((version, (links, colors), noticed)),
            _ => {},
        }
    }
}

/// What answered resolve `request`.
async fn resolved(
    events: &mut EventRx,
    request: RequestId,
) -> Option<(DocumentId, Result<DocumentLink, String>)> {
    while let Some((id, event)) = next_event(events).await {
        if let Event::DocumentLinkResolved { doc, result } = event
            && id == Some(request)
        {
            return Some((doc, result));
        }
    }
    None
}

fn ask(
    backend: &impl Backend,
    doc: DocumentId,
) -> Result<RequestId, Box<dyn std::error::Error + Send + Sync>> {
    let request = backend.next_id();
    backend.send(request, Command::LinksAndColors { doc })?;
    Ok(request)
}

fn span(line: u32, start: u32, end: u32) -> Result<Range, String> {
    Range::new(LineCol::new(line, start), LineCol::new(line, end)).map_err(|e| format!("{e}"))
}

#[tokio::test]
async fn links_and_colours_arrive_in_buffer_columns() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "main.rs", Behavior::Normal).await?;

    let request = ask(&backend, doc)?;
    let (at, (links, colors), noticed) = listing(&mut events, request).await?;

    assert_eq!(at, version);
    assert!(!noticed);
    let links = links.ok_or("the link half failed")?;
    let [web, deferred] = links.as_slice() else {
        return Err(format!("expected two links, got {links:?}").into());
    };
    // The server's UTF-16 2..4 is buffer columns 1..3, after the emoji.
    assert_eq!(web.range, span(0, 1, 3)?);
    assert_eq!(web.target.as_deref(), Some("https://example.com/"));
    assert_eq!(web.tooltip.as_deref(), Some("Example"));
    assert!(!deferred.is_resolved());
    let colors = colors.ok_or("the colour half failed")?;
    assert_eq!(
        colors,
        [DocumentColor {
            range: span(0, 1, 3)?,
            rgba: [255, 0, 0, 255],
        }]
    );
    Ok(())
}

/// A deferred link resolves through the server with its own data, keeping
/// the range the client holds it at; one the server leaves without a target
/// fails saying so.
#[tokio::test]
async fn a_deferred_link_resolves_through_the_server() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _) = opened(&dir, "main.rs", Behavior::Normal).await?;
    let request = ask(&backend, doc)?;
    let (_, (links, _), _) = listing(&mut events, request).await?;
    let deferred = links
        .and_then(|links| links.into_iter().nth(1))
        .ok_or("the deferred link")?;

    let resolve = backend.next_id();
    backend.send(
        resolve,
        Command::ResolveDocumentLink {
            doc,
            link: deferred.clone(),
        },
    )?;
    let (answered, result) = resolved(&mut events, resolve).await.ok_or("no answer")?;
    assert_eq!(answered, doc);
    let link = result?;
    assert_eq!(link.target.as_deref(), Some("file:///w/2.rs"));
    assert_eq!(link.range, deferred.range);

    let mut targetless = deferred;
    let mut literal: Value = serde_json::from_str(targetless.payload.as_deref().unwrap_or("{}"))?;
    literal["data"]["n"] = json!(0);
    targetless.payload = Some(literal.to_string());
    let resolve = backend.next_id();
    backend.send(
        resolve,
        Command::ResolveDocumentLink {
            doc,
            link: targetless,
        },
    )?;
    let (_, result) = resolved(&mut events, resolve).await.ok_or("no answer")?;
    assert_eq!(result, Err(crate::lsp::NO_TARGET.to_owned()));
    Ok(())
}

/// A link that already knows its target is answered as it is, and one with
/// nothing to hand a server back fails at once -- neither asks anyone, so
/// neither needs a server.
#[tokio::test]
async fn a_link_with_nothing_to_resolve_is_answered_without_a_server() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, _) = opened(&dir, "notes.txt", Behavior::Normal).await?;
    let known = DocumentLink {
        target: Some("https://example.com/".into()),
        ..DocumentLink::default()
    };
    let resolve = backend.next_id();
    backend.send(
        resolve,
        Command::ResolveDocumentLink {
            doc,
            link: known.clone(),
        },
    )?;
    let (_, result) = resolved(&mut events, resolve).await.ok_or("no answer")?;
    assert_eq!(result, Ok(known));

    let resolve = backend.next_id();
    backend.send(
        resolve,
        Command::ResolveDocumentLink {
            doc,
            link: DocumentLink::default(),
        },
    )?;
    let (_, result) = resolved(&mut events, resolve).await.ok_or("no answer")?;
    assert_eq!(result, Err(crate::lsp::NO_TARGET.to_owned()));
    Ok(())
}

#[tokio::test]
async fn a_server_without_links_or_colours_answers_none_and_says_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "main.rs", Behavior::Bare).await?;

    let request = ask(&backend, doc)?;
    let (at, halves, noticed) = listing(&mut events, request).await?;

    assert_eq!(
        (at, halves),
        (version, (Some(Vec::new()), Some(Vec::new())))
    );
    assert!(!noticed, "they are asked unprompted; a refusal is no news");
    Ok(())
}

#[tokio::test]
async fn a_document_no_server_serves_answers_none_of_either() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc, version) = opened(&dir, "notes.txt", Behavior::Normal).await?;

    let request = ask(&backend, doc)?;
    let (at, halves, _) = listing(&mut events, request).await?;

    assert_eq!(
        (at, halves),
        (version, (Some(Vec::new()), Some(Vec::new())))
    );
    let resolve = backend.next_id();
    let deferred = DocumentLink {
        payload: Some("{}".into()),
        ..DocumentLink::default()
    };
    backend.send(
        resolve,
        Command::ResolveDocumentLink {
            doc,
            link: deferred,
        },
    )?;
    let (_, result) = resolved(&mut events, resolve).await.ok_or("no answer")?;
    assert!(result.is_err(), "no server resolves it: {result:?}");
    Ok(())
}

/// A session with `notes.txt` (holding [`TEXT`]) open as document 1, and
/// that document's version.
fn session_with_notes(
    dir: &tempfile::TempDir,
) -> Result<(Session, EventRx, DocumentId, u64), Box<dyn std::error::Error + Send + Sync>> {
    let path = rust_file(dir, "notes.txt", TEXT).ok_or("write failed")?;
    let (mut session, events, _snaps) = Session::new(SessionConfig::default());
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
    Ok((session, events, doc, version))
}

fn listed(request: RequestId, doc: DocumentId, version: u64) -> LinkColorAnswer {
    LinkColorAnswer::Listed {
        request,
        doc,
        version,
        links: Some(vec![DocumentLink::default()]),
        colors: Some(vec![DocumentColor::default()]),
    }
}

/// The halves `request` was answered with, draining every event.
fn answered(events: &mut EventRx, request: RequestId) -> Option<(u64, Halves)> {
    let mut answer = None;
    while let Some((id, event)) = events.try_recv() {
        if let Event::LinksAndColors {
            version,
            links,
            colors,
            ..
        } = event
            && id == Some(request)
        {
            answer = Some((version, (links, colors)));
        }
    }
    answer
}

/// A listing begun under a settings snapshot `reconfigure` has since retired
/// is fenced out, but still answered -- with nothing learned, so the client
/// asks again rather than waiting, and keeps what it shows.
#[test]
fn a_listing_from_a_retired_generation_is_answered_unanswered() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (mut session, mut events, doc, version) = session_with_notes(&dir)?;

    session.apply_lsp_update(LspUpdate::LinkColor {
        generation: u64::MAX,
        answer: listed(RequestId(2), doc, version),
    });

    assert_eq!(
        answered(&mut events, RequestId(2)),
        Some((version, (None, None)))
    );
    Ok(())
}

/// A listing answered for a version the buffer has since moved past learns
/// nothing: its ranges describe text that is gone. The same answer at the
/// current version is adopted, so it is the version that failed it.
#[test]
fn a_listing_for_a_superseded_version_learns_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (mut session, mut events, doc, old) = session_with_notes(&dir)?;
    session.handle(
        RequestId(2),
        Command::ApplyChange {
            doc,
            change: Change::new(
                old,
                vec![TextEdit {
                    range: span(1, 0, 0)?,
                    new_text: "x".to_owned(),
                }],
            ),
            cause: EditCause::Type,
        },
    );
    let current = session
        .document(doc)
        .map(|d| d.version())
        .ok_or("no document")?;
    assert_ne!(current, old, "the edit moved the version on");
    while events.try_recv().is_some() {}

    session.apply_lsp_update(LspUpdate::LinkColor {
        generation: 0,
        answer: listed(RequestId(3), doc, old),
    });
    assert_eq!(
        answered(&mut events, RequestId(3)),
        Some((old, (None, None)))
    );
    session.apply_lsp_update(LspUpdate::LinkColor {
        generation: 0,
        answer: listed(RequestId(4), doc, current),
    });
    let fresh = answered(&mut events, RequestId(4)).map(|(version, (links, colors))| {
        (
            version,
            links.map(|links| links.len()),
            colors.map(|colors| colors.len()),
        )
    });
    assert_eq!(fresh, Some((current, Some(1), Some(1))));
    Ok(())
}
