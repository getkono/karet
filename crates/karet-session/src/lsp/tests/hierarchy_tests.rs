//! The seam contract for [`Command::PrepareHierarchy`] and
//! [`Command::ExpandHierarchy`]: a server that offers the hierarchy answers
//! with items in buffer columns, an item handed back to grow the tree reaches
//! the server exactly as it gave it, and every other case -- a server without
//! the hierarchy, no server at all -- still answers, with no items.

use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;

use super::*;

/// Both lines open with an emoji: one buffer column, two UTF-16 code units,
/// so a column that survives the round trip unconverted lands one off.
const TEXT: &str = "😀abcd\n😀fn main() {}\n";

type Answer = (HierarchyDirection, Vec<HierarchyItem>, bool);

/// A session over `name` (holding [`TEXT`]) served by a double that behaves
/// as `behavior`, with the document open.
async fn opened(
    dir: &tempfile::TempDir,
    name: &str,
    behavior: Behavior,
) -> Result<(impl Backend, EventRx, DocumentId), Box<dyn std::error::Error + Send + Sync>> {
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
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    Ok((backend, events, doc))
}

/// What answered `request`: the [`Event::Hierarchy`] fields, and whether a
/// [`Event::FeatureUnsupported`] came for it first.
async fn answer_to(
    events: &mut EventRx,
    request: RequestId,
) -> Result<Answer, Box<dyn std::error::Error + Send + Sync>> {
    let mut noticed = false;
    loop {
        let (id, event) = next_event(events).await.ok_or("no Hierarchy answer")?;
        match event {
            Event::FeatureUnsupported { .. } if id == Some(request) => noticed = true,
            Event::Hierarchy { direction, items } if id == Some(request) => {
                return Ok((direction, items, noticed));
            },
            _ => {},
        }
    }
}

/// Prepare at `b` on line 0 (buffer column 2) and wait for the answer.
async fn prepare(
    backend: &impl Backend,
    events: &mut EventRx,
    doc: DocumentId,
    direction: HierarchyDirection,
) -> Result<Answer, Box<dyn std::error::Error + Send + Sync>> {
    let request = backend.next_id();
    let position = LineCol::new(0, 2);
    backend.send(
        request,
        Command::PrepareHierarchy {
            doc,
            position,
            direction,
        },
    )?;
    answer_to(events, request).await
}

/// Expand `item` in `direction` and wait for the answer.
async fn expand(
    backend: &impl Backend,
    events: &mut EventRx,
    doc: DocumentId,
    (item, direction): (HierarchyItem, HierarchyDirection),
) -> Result<Answer, Box<dyn std::error::Error + Send + Sync>> {
    let request = backend.next_id();
    let item = Box::new(item);
    backend.send(
        request,
        Command::ExpandHierarchy {
            doc,
            item,
            direction,
        },
    )?;
    answer_to(events, request).await
}

/// The root comes back in buffer columns (the double echoes the caret's
/// UTF-16 column, so `b` is buffer column 2 only if it went out as UTF-16
/// column 3 and was converted back), and expanding it hands the item back
/// with the `data` the double gave it: a payload rebuilt from the neutral
/// fields would come back named `... of null`.
#[tokio::test]
async fn a_root_comes_back_in_buffer_columns_and_grows_from_its_own_data() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc) = opened(&dir, "main.rs", Behavior::Normal).await?;

    let incoming = HierarchyDirection::IncomingCalls;
    let (direction, roots, noticed) = prepare(&backend, &mut events, doc, incoming).await?;
    assert_eq!(direction, incoming);
    assert!(!noticed, "a server that offers it was reported as not");
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].name, "root");
    assert_eq!(roots[0].location.range.start, LineCol::new(0, 2));

    let root = roots[0].clone();
    let (_, callers, _) = expand(&backend, &mut events, doc, (root, incoming)).await?;
    assert_eq!(callers.len(), 1);
    assert_eq!(
        callers[0].name,
        "callHierarchy/incomingCalls of \"textDocument/prepareCallHierarchy\""
    );
    // UTF-16 character 2 of line 1 is just past its emoji: buffer column 1.
    assert_eq!(callers[0].location.range.start, LineCol::new(1, 1));
    Ok(())
}

/// Every direction reaches the method that walks it, from the prepare of
/// its own hierarchy.
#[tokio::test]
async fn each_direction_grows_by_its_own_method() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc) = opened(&dir, "main.rs", Behavior::Normal).await?;
    for (direction, expected) in [
        (
            HierarchyDirection::OutgoingCalls,
            "callHierarchy/outgoingCalls of \"textDocument/prepareCallHierarchy\"",
        ),
        (
            HierarchyDirection::Supertypes,
            "typeHierarchy/supertypes of \"textDocument/prepareTypeHierarchy\"",
        ),
        (
            HierarchyDirection::Subtypes,
            "typeHierarchy/subtypes of \"textDocument/prepareTypeHierarchy\"",
        ),
    ] {
        let (_, roots, _) = prepare(&backend, &mut events, doc, direction).await?;
        let root = roots.into_iter().next().ok_or("no root")?;
        let (answered, children, _) = expand(&backend, &mut events, doc, (root, direction)).await?;
        assert_eq!(answered, direction);
        let names: Vec<_> = children.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec![expected], "{direction:?}");
    }
    Ok(())
}

/// A server that does not offer the hierarchy is reported, ahead of an
/// empty answer the client is still waiting on.
#[tokio::test]
async fn a_server_without_the_hierarchy_says_so_and_answers_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc) = opened(&dir, "main.rs", Behavior::Bare).await?;

    let types = HierarchyDirection::Subtypes;
    let (direction, items, noticed) = prepare(&backend, &mut events, doc, types).await?;
    assert_eq!(direction, types);
    assert!(items.is_empty());
    assert!(
        noticed,
        "the refusal was not surfaced ahead of its empty answer"
    );
    Ok(())
}

/// A document no server serves is answered at once, with no items and no
/// notice: there is no server to blame.
#[tokio::test]
async fn a_document_no_server_serves_answers_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (backend, mut events, doc) = opened(&dir, "notes.txt", Behavior::Normal).await?;

    let calls = HierarchyDirection::IncomingCalls;
    let (_, items, noticed) = prepare(&backend, &mut events, doc, calls).await?;
    assert!(items.is_empty());
    assert!(!noticed);
    Ok(())
}

/// An answer begun under a settings snapshot that `reconfigure` has since
/// retired is fenced out, but the request it answers is still answered --
/// with no items -- so the client's tree is not left waiting on it.
#[test]
fn an_answer_from_a_retired_generation_still_answers_empty() {
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let item = HierarchyItem {
        name: "stale".to_owned(),
        kind: karet_core::SymbolKind::Function,
        detail: None,
        location: karet_core::Location {
            path: "/nowhere.rs".into(),
            range: Range::default(),
        },
        payload: "{}".to_owned(),
    };
    session.apply_lsp_update(LspUpdate::Hierarchy {
        generation: u64::MAX,
        request: RequestId(2),
        direction: HierarchyDirection::Supertypes,
        items: vec![item],
    });

    let mut answer = None;
    while let Some((id, event)) = events.try_recv() {
        if let (Some(RequestId(2)), Event::Hierarchy { items, .. }) = (id, event) {
            answer = Some(items);
        }
    }
    assert_eq!(answer, Some(Vec::new()));
}
